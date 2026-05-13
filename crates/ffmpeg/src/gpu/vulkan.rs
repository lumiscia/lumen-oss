use crate::gpu::GpuBackend;

pub const BACKEND: GpuBackend = GpuBackend::Vulkan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VulkanVideoFrame {
    image: ash::vk::Image,
    image_view: ash::vk::ImageView,
    memory: ash::vk::DeviceMemory,
    format: ash::vk::Format,
    extent: ash::vk::Extent3D,
}

impl VulkanVideoFrame {
    pub fn new(
        image: ash::vk::Image,
        image_view: ash::vk::ImageView,
        memory: ash::vk::DeviceMemory,
        format: ash::vk::Format,
        extent: ash::vk::Extent3D,
    ) -> Self {
        Self {
            image,
            image_view,
            memory,
            format,
            extent,
        }
    }

    pub fn image(&self) -> ash::vk::Image {
        self.image
    }

    pub fn image_view(&self) -> ash::vk::ImageView {
        self.image_view
    }

    pub fn memory(&self) -> ash::vk::DeviceMemory {
        self.memory
    }

    pub fn format(&self) -> ash::vk::Format {
        self.format
    }

    pub fn extent(&self) -> ash::vk::Extent3D {
        self.extent
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }

    pub fn pts(&self) -> Option<i64> {
        None
    }

    pub fn backend(&self) -> GpuBackend {
        GpuBackend::Vulkan
    }
}
