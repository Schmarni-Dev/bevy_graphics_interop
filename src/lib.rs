#[cfg(feature = "vulkan")]
pub mod vulkan_init;

use core::ffi::CStr;

use bevy_ecs::resource::Resource;

// needs to be inserted before a compatible plugin tries to init graphics
#[derive(Default, Resource)]
pub struct GraphicsInteropSettings {
    #[cfg(feature = "vulkan")]
    vulkan_instance_extensions: Vec<&'static CStr>,
    #[cfg(feature = "vulkan")]
    vulkan_device_extensions: Vec<&'static CStr>,
}
impl GraphicsInteropSettings {
    #[cfg(feature = "vulkan")]
    pub fn add_vk_instance_extension(&mut self, extension: &'static CStr) {
        if !self.vulkan_instance_extensions.contains(&extension) {
            self.vulkan_instance_extensions.push(extension);
        }
    }
    #[cfg(feature = "vulkan")]
    pub fn add_vk_device_extension(&mut self, extension: &'static CStr) {
        if !self.vulkan_device_extensions.contains(&extension) {
            self.vulkan_device_extensions.push(extension);
        }
    }
    #[cfg(feature = "vulkan")]
    pub fn read_vk_instance_extensions(&self) -> &[&'static CStr] {
        &self.vulkan_instance_extensions
    }
    #[cfg(feature = "vulkan")]
    pub fn read_vk_device_extensions(&self) -> &[&'static CStr] {
        &self.vulkan_instance_extensions
    }
}
