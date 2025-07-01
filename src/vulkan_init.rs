pub struct VulkanWgpuInitPlugin;

pub fn add_dmabuf_init_plugin<G: PluginGroup>(plugins: G) -> PluginGroupBuilder {
    plugins
        .build()
        .disable::<RenderPlugin>()
        .add_before::<RenderPlugin>(VulkanWgpuInitPlugin)
}

impl Plugin for VulkanWgpuInitPlugin {
    fn build(&self, app: &mut bevy_app::App) {
        let Some(settings) = app.world_mut().remove_resource::<GraphicsInteropSettings>() else {
            return;
        };
        let (device, queue, adapter_info, adapter, instance) = init_graphics(settings).unwrap();
        app.add_plugins(RenderPlugin {
            render_creation: RenderCreation::Manual(RenderResources(
                device.into(),
                RenderQueue(Arc::new(WgpuWrapper::new(queue))),
                RenderAdapterInfo(WgpuWrapper::new(adapter_info)),
                RenderAdapter(Arc::new(WgpuWrapper::new(adapter))),
                RenderInstance(Arc::new(WgpuWrapper::new(instance))),
            )),
            synchronous_pipeline_compilation: false,
            debug_flags: RenderDebugFlags::default(),
        });
    }
}

use std::sync::Arc;

use ash::vk::PhysicalDeviceType;
use bevy_app::{Plugin, PluginGroup, PluginGroupBuilder};
use bevy_render::renderer::{
    RenderAdapter, RenderAdapterInfo, RenderInstance, RenderQueue, WgpuWrapper,
};
use bevy_render::settings::{RenderCreation, RenderResources};
use bevy_render::{RenderDebugFlags, RenderPlugin};
use thiserror::Error;
use wgpu::hal::Api;
use wgpu::hal::api::Vulkan;

use crate::GraphicsInteropSettings;

#[cfg(not(target_os = "android"))]
const VK_TARGET_VERSION_ASH: u32 = ash::vk::make_api_version(0, 1, 2, 0);
#[cfg(target_os = "android")]
const VK_TARGET_VERSION_ASH: u32 = ash::vk::make_api_version(0, 1, 1, 0);

fn init_graphics(
    settings: GraphicsInteropSettings,
) -> Result<
    (
        wgpu::Device,
        wgpu::Queue,
        wgpu::AdapterInfo,
        wgpu::Adapter,
        wgpu::Instance,
    ),
    VulkanInitError,
> {
    let vk_entry = unsafe { ash::Entry::load() }?;
    let flags = wgpu::InstanceFlags::default().with_env();
    let mut instance_extensions =
        <Vulkan as Api>::Instance::desired_extensions(&vk_entry, VK_TARGET_VERSION_ASH, flags)?;
    instance_extensions.dedup();
    let device_extensions = settings.read_vk_device_extensions();

    let vk_instance = unsafe {
        let extensions_cchar: Vec<_> = instance_extensions.iter().map(|s| s.as_ptr()).collect();

        let app_name = c"bevy_graphics_interop vulkan app";
        let vk_app_info = ash::vk::ApplicationInfo::default()
            .application_name(app_name)
            .application_version(1)
            .engine_name(c"bevy")
            .engine_version(15)
            .api_version(VK_TARGET_VERSION_ASH);

        vk_entry.create_instance(
            &ash::vk::InstanceCreateInfo::default()
                .application_info(&vk_app_info)
                .enabled_extension_names(&extensions_cchar),
            None,
        )?
    };
    let api_layers = unsafe { vk_entry.enumerate_instance_layer_properties()? };
    let has_nv_optimus = api_layers.iter().any(|v| {
        v.layer_name_as_c_str()
            .is_ok_and(|v| v == c"VK_LAYER_NV_optimus")
    });

    drop(api_layers);
    let version = { unsafe { vk_entry.try_enumerate_instance_version()? } };
    let instance_api_version = match version {
        // Vulkan 1.1+
        Some(version) => version,
        None => ash::vk::API_VERSION_1_0,
    };

    // the android_sdk_version stuff is copied from wgpu
    #[cfg(target_os = "android")]
    let android_sdk_version = {
        let properties = android_system_properties::AndroidSystemProperties::new();
        // See: https://developer.android.com/reference/android/os/Build.VERSION_CODES
        if let Some(val) = properties.get("ro.build.version.sdk") {
            match val.parse::<u32>() {
                Ok(sdk_ver) => sdk_ver,
                Err(err) => {
                    error!(
                        concat!(
                            "Couldn't parse Android's ",
                            "ro.build.version.sdk system property ({}): {}",
                        ),
                        val, err,
                    );
                    0
                }
            }
        } else {
            error!("Couldn't read Android's ro.build.version.sdk system property");
            0
        }
    };
    #[cfg(not(target_os = "android"))]
    let android_sdk_version = 0;

    let wgpu_vk_instance = unsafe {
        <Vulkan as Api>::Instance::from_raw(
            vk_entry.clone(),
            vk_instance.clone(),
            instance_api_version,
            android_sdk_version,
            None,
            instance_extensions,
            flags,
            has_nv_optimus,
            None,
        )?
    };
    let vk_physical_device = {
        let mut devices = unsafe { vk_instance.enumerate_physical_devices()? };
        devices.sort_by_key(|physical_device| {
            match unsafe {
                vk_instance
                    .get_physical_device_properties(*physical_device)
                    .device_type
            } {
                PhysicalDeviceType::DISCRETE_GPU => 1,
                PhysicalDeviceType::INTEGRATED_GPU => 2,
                PhysicalDeviceType::OTHER => 3,
                PhysicalDeviceType::VIRTUAL_GPU => 4,
                PhysicalDeviceType::CPU => 5,
                _ => 6,
            }
        });
        let Some(phys_dev) = devices.into_iter().next() else {
            return Err(VulkanInitError::NoPhysicalDevice);
        };
        phys_dev
    };
    let Some(wgpu_exposed_adapter) = wgpu_vk_instance.expose_adapter(vk_physical_device) else {
        return Err(VulkanInitError::NoWgpuAdapter);
    };
    let wgpu_features = wgpu_exposed_adapter.features;

    let enabled_extensions = wgpu_exposed_adapter
        .adapter
        .required_device_extensions(wgpu_features);

    let wgpu_open_device = {
        let extensions_cchar: Vec<_> = device_extensions.iter().map(|s| s.as_ptr()).collect();
        let mut enabled_phd_features = wgpu_exposed_adapter
            .adapter
            .physical_device_features(&enabled_extensions, wgpu_features);
        let family_index = 0;
        let family_info = ash::vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family_index)
            .queue_priorities(&[1.0]);
        let family_infos = [family_info];
        let mut physical_device_multiview_features = ash::vk::PhysicalDeviceMultiviewFeatures {
            multiview: ash::vk::TRUE,
            ..Default::default()
        };
        let info = enabled_phd_features
            .add_to_device_create(
                ash::vk::DeviceCreateInfo::default()
                    .queue_create_infos(&family_infos)
                    .push_next(&mut physical_device_multiview_features),
            )
            .enabled_extension_names(&extensions_cchar);
        let vk_device = unsafe { vk_instance.create_device(vk_physical_device, &info, None)? };

        unsafe {
            wgpu_exposed_adapter.adapter.device_from_raw(
                vk_device,
                None,
                &enabled_extensions,
                wgpu_features,
                &wgpu::MemoryHints::Performance,
                family_info.queue_family_index,
                0,
            )
        }?
    };

    let wgpu_instance =
        unsafe { wgpu::Instance::from_hal::<wgpu::hal::api::Vulkan>(wgpu_vk_instance) };
    let wgpu_adapter = unsafe { wgpu_instance.create_adapter_from_hal(wgpu_exposed_adapter) };
    let limits = wgpu_adapter.limits();
    let (wgpu_device, wgpu_queue) = unsafe {
        wgpu_adapter.create_device_from_hal(
            wgpu_open_device,
            &wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu_features,
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        )
    }?;

    Ok((
        wgpu_device,
        wgpu_queue,
        wgpu_adapter.get_info(),
        wgpu_adapter,
        wgpu_instance,
    ))
}
#[derive(Error, Debug)]
pub enum VulkanInitError {
    #[error("No Physical Vulkan Device")]
    NoPhysicalDevice,
    #[error("No Wgpu adapter could be provided for the Physical Device")]
    NoWgpuAdapter,
    #[error("unable to load vulkan: {0}")]
    AshLoadingErr(#[from] ash::LoadingError),
    #[error("vulkan error: {0}")]
    AshErr(#[from] ash::vk::Result),
    #[error("wgpu_hal instance error: {0}")]
    WgpuHalInstanceErr(#[from] wgpu::hal::InstanceError),
    #[error("wgpu_hal device error: {0}")]
    WgpuHalDeviceErr(#[from] wgpu::hal::DeviceError),
    #[error("failed to request wgpu device: {0}")]
    WgpuRequestDeviceErr(#[from] wgpu::RequestDeviceError),
}
