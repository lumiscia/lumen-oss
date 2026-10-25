#[cfg(feature = "cuda")]
pub mod cuda;
#[cfg(feature = "metal")]
pub mod metal;
#[cfg(feature = "vulkan")]
pub mod vulkan;

#[cfg(feature = "cuda")]
pub use cuda::CudaDecodedFrame;
#[cfg(all(feature = "cuda", target_os = "linux"))]
pub use cuda::{
    CudaContext, CudaDeviceAllocation, CudaDriver, CudaNv12ToRgbaConverter,
    ImportedCudaExternalImage, import_owned_vulkan_opaque_fd_image, import_vulkan_opaque_fd_image,
};
#[cfg(feature = "metal")]
pub use metal::{
    MetalDecodedFrame, MetalPixelBufferFrame, MetalPixelBufferPool, MetalTextureCache,
    Objc2MetalDevice, Objc2MetalTexture,
};
#[cfg(feature = "vulkan")]
pub use vulkan::VulkanVideoFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    Cuda,
    Metal,
    Vulkan,
}

#[derive(Debug)]
pub enum GpuVideoInput<'a> {
    #[cfg(feature = "cuda")]
    Cuda(&'a CudaVideoFrame),
    #[cfg(feature = "metal")]
    Metal(&'a Objc2MetalTexture),
    #[cfg(feature = "metal")]
    MetalPixelBuffer(&'a MetalPixelBufferFrame),
    #[cfg(feature = "vulkan")]
    Vulkan(&'a VulkanVideoFrame),
    #[doc(hidden)]
    __NonExhaustive(std::marker::PhantomData<&'a ()>),
}

impl<'a> GpuVideoInput<'a> {
    pub fn backend(&self) -> GpuBackend {
        match self {
            #[cfg(feature = "cuda")]
            Self::Cuda(_) => GpuBackend::Cuda,
            #[cfg(feature = "metal")]
            Self::Metal(_) | Self::MetalPixelBuffer(_) => GpuBackend::Metal,
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => GpuBackend::Vulkan,
            Self::__NonExhaustive(_) => unreachable!("hidden GPU frame variant"),
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            #[cfg(feature = "cuda")]
            Self::Cuda(frame) => frame.dimensions(),
            #[cfg(feature = "metal")]
            Self::Metal(texture) => {
                use objc2_metal::MTLTexture;

                (texture.width() as u32, texture.height() as u32)
            }
            #[cfg(feature = "metal")]
            Self::MetalPixelBuffer(frame) => frame.dimensions(),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(frame) => frame.dimensions(),
            Self::__NonExhaustive(_) => unreachable!("hidden GPU frame variant"),
        }
    }

    pub fn pts(&self) -> Option<i64> {
        match self {
            #[cfg(feature = "cuda")]
            Self::Cuda(frame) => frame.pts(),
            #[cfg(feature = "metal")]
            Self::Metal(_) => None,
            #[cfg(feature = "metal")]
            Self::MetalPixelBuffer(frame) => frame.pts(),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(frame) => frame.pts(),
            Self::__NonExhaustive(_) => unreachable!("hidden GPU frame variant"),
        }
    }

    pub fn estimated_rgba_bytes(&self) -> u64 {
        let (width, height) = self.dimensions();
        u64::from(width)
            .saturating_mul(u64::from(height))
            .saturating_mul(4)
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum GpuVideoFrame {
    #[cfg(feature = "cuda")]
    Cuda(CudaDecodedFrame),
    #[cfg(feature = "metal")]
    Metal(MetalDecodedFrame),
    #[cfg(feature = "vulkan")]
    Vulkan(VulkanVideoFrame),
    #[doc(hidden)]
    __NonExhaustive,
}

impl GpuVideoFrame {
    pub fn backend(&self) -> GpuBackend {
        match self {
            #[cfg(feature = "cuda")]
            Self::Cuda(_) => GpuBackend::Cuda,
            #[cfg(feature = "metal")]
            Self::Metal(_) => GpuBackend::Metal,
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => GpuBackend::Vulkan,
            Self::__NonExhaustive => unreachable!("hidden GPU frame variant"),
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            #[cfg(feature = "cuda")]
            Self::Cuda(frame) => frame.dimensions(),
            #[cfg(feature = "metal")]
            Self::Metal(frame) => frame.dimensions(),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(frame) => frame.dimensions(),
            Self::__NonExhaustive => unreachable!("hidden GPU frame variant"),
        }
    }

    pub fn pts(&self) -> Option<i64> {
        match self {
            #[cfg(feature = "cuda")]
            Self::Cuda(frame) => frame.pts(),
            #[cfg(feature = "metal")]
            Self::Metal(frame) => frame.pts(),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(frame) => frame.pts(),
            Self::__NonExhaustive => unreachable!("hidden GPU frame variant"),
        }
    }

    pub fn estimated_rgba_bytes(&self) -> u64 {
        let (width, height) = self.dimensions();
        u64::from(width)
            .saturating_mul(u64::from(height))
            .saturating_mul(4)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "cuda")]
    #[test]
    fn cuda_video_frame_reports_shape() {
        use super::*;

        // SAFETY: This metadata-only test never dereferences or submits the fake device pointer.
        let frame = unsafe { CudaVideoFrame::from_device_ptr(0xCAFE, 1920, 1080, 8192, Some(7)) };
        let input = GpuVideoInput::Cuda(&frame);

        assert_eq!(input.backend(), GpuBackend::Cuda);
        assert_eq!(input.dimensions(), (1920, 1080));
        assert_eq!(frame.device_ptr(), 0xCAFE);
        assert_eq!(frame.pitch(), 8192);
        assert_eq!(frame.pts(), Some(7));
    }

    #[cfg(feature = "vulkan")]
    #[test]
    fn vulkan_video_frame_reports_shape() {
        use ash::vk::Handle;

        use super::*;

        let frame = VulkanVideoFrame::new(
            ash::vk::Image::from_raw(11),
            ash::vk::ImageView::from_raw(22),
            ash::vk::DeviceMemory::from_raw(33),
            ash::vk::Format::R8G8B8A8_UNORM,
            ash::vk::Extent3D {
                width: 640,
                height: 480,
                depth: 1,
            },
        );
        let input = GpuVideoInput::Vulkan(&frame);

        assert_eq!(input.backend(), GpuBackend::Vulkan);
        assert_eq!(input.dimensions(), (640, 480));
        assert_eq!(frame.image().as_raw(), 11);
        assert_eq!(frame.image_view().as_raw(), 22);
        assert_eq!(frame.memory().as_raw(), 33);
        assert_eq!(frame.format(), ash::vk::Format::R8G8B8A8_UNORM);
    }
}
#[cfg(feature = "cuda")]
pub use cuda::{
    CudaExternalMemoryHandle, CudaExternalSemaphoreHandle, CudaVideoFrame, VulkanToCudaExport,
};
