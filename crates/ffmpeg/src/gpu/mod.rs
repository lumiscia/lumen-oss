#[cfg(feature = "metal")]
pub mod metal;
#[cfg(feature = "vulkan")]
pub mod vulkan;

#[cfg(feature = "metal")]
use std::ptr::NonNull;

#[cfg(feature = "metal")]
use objc2::rc::Retained;
#[cfg(feature = "metal")]
use objc2_core_foundation::CFRetained;

#[cfg(feature = "metal")]
use crate::{FfmpegError, Result};
#[cfg(feature = "metal")]
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane,
    kCVReturnSuccess,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    Metal,
    Vulkan,
}

#[cfg(feature = "metal")]
pub type Objc2MetalTexture = objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>;

#[cfg(feature = "metal")]
pub type Objc2MetalDevice = objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>;

#[derive(Debug)]
pub enum GpuVideoInput<'a> {
    #[cfg(feature = "metal")]
    Metal(&'a Objc2MetalTexture),
    #[cfg(feature = "vulkan")]
    Vulkan {
        image: ash::vk::Image,
        image_view: ash::vk::ImageView,
        memory: ash::vk::DeviceMemory,
        format: ash::vk::Format,
        extent: ash::vk::Extent3D,
    },
    #[doc(hidden)]
    __NonExhaustive(std::marker::PhantomData<&'a ()>),
}

impl<'a> GpuVideoInput<'a> {
    pub fn backend(&self) -> GpuBackend {
        match self {
            #[cfg(feature = "metal")]
            Self::Metal(_) => GpuBackend::Metal,
            #[cfg(feature = "vulkan")]
            Self::Vulkan { .. } => GpuBackend::Vulkan,
            Self::__NonExhaustive(_) => unreachable!("hidden GPU frame variant"),
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            #[cfg(feature = "metal")]
            Self::Metal(texture) => {
                use objc2_metal::MTLTexture;

                (texture.width() as u32, texture.height() as u32)
            }
            #[cfg(feature = "vulkan")]
            Self::Vulkan { extent, .. } => (extent.width, extent.height),
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
    #[cfg(feature = "metal")]
    Metal(MetalDecodedFrame),
}

impl GpuVideoFrame {
    pub fn backend(&self) -> GpuBackend {
        #[cfg(feature = "metal")]
        {
            match self {
                Self::Metal(_) => GpuBackend::Metal,
            }
        }
        #[cfg(not(feature = "metal"))]
        {
            match *self {}
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        #[cfg(feature = "metal")]
        {
            match self {
                Self::Metal(frame) => frame.dimensions(),
            }
        }
        #[cfg(not(feature = "metal"))]
        {
            match *self {}
        }
    }

    pub fn estimated_rgba_bytes(&self) -> u64 {
        let (width, height) = self.dimensions();
        u64::from(width)
            .saturating_mul(u64::from(height))
            .saturating_mul(4)
    }
}

#[cfg(feature = "metal")]
pub struct MetalTextureCache {
    inner: CFRetained<CVMetalTextureCache>,
}

#[cfg(feature = "metal")]
impl std::fmt::Debug for MetalTextureCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalTextureCache")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "metal")]
impl MetalTextureCache {
    pub fn create(device: &Objc2MetalDevice) -> Result<Self> {
        let mut cache = std::ptr::null_mut();
        let status = unsafe {
            CVMetalTextureCache::create(
                None,
                None,
                device,
                None,
                NonNull::new_unchecked(&mut cache),
            )
        };
        if status != kCVReturnSuccess {
            return Err(FfmpegError::new(
                "CVMetalTextureCacheCreate",
                format!("CoreVideo returned {status}"),
            )
            .with_backend(GpuBackend::Metal));
        }
        let cache = NonNull::new(cache).ok_or_else(|| {
            FfmpegError::new(
                "CVMetalTextureCacheCreate",
                "CoreVideo returned a null cache",
            )
            .with_backend(GpuBackend::Metal)
        })?;
        Ok(Self {
            inner: unsafe { CFRetained::from_raw(cache) },
        })
    }

    pub fn flush(&self) {
        self.inner.flush(0);
    }
}

#[cfg(feature = "metal")]
pub struct MetalDecodedFrame {
    pixel_buffer: CFRetained<CVPixelBuffer>,
    width: u32,
    height: u32,
    pixel_format: u32,
    pts: Option<i64>,
}

#[cfg(feature = "metal")]
impl std::fmt::Debug for MetalDecodedFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalDecodedFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixel_format", &self.pixel_format)
            .field("pts", &self.pts)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "metal")]
impl MetalDecodedFrame {
    pub(crate) unsafe fn retain_from_video_toolbox_frame(
        pixel_buffer: NonNull<CVPixelBuffer>,
        pts: Option<i64>,
    ) -> Self {
        let pixel_buffer = unsafe { CFRetained::retain(pixel_buffer) };
        let width = CVPixelBufferGetWidth(&pixel_buffer).min(u32::MAX as usize) as u32;
        let height = CVPixelBufferGetHeight(&pixel_buffer).min(u32::MAX as usize) as u32;
        let pixel_format = CVPixelBufferGetPixelFormatType(&pixel_buffer);
        Self {
            pixel_buffer,
            width,
            height,
            pixel_format,
            pts,
        }
    }

    pub fn pixel_buffer(&self) -> &CVPixelBuffer {
        &self.pixel_buffer
    }

    pub fn pixel_format(&self) -> u32 {
        self.pixel_format
    }

    pub fn plane_count(&self) -> usize {
        CVPixelBufferGetPlaneCount(&self.pixel_buffer)
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pts(&self) -> Option<i64> {
        self.pts
    }

    pub fn create_texture(
        &self,
        cache: &MetalTextureCache,
        pixel_format: objc2_metal::MTLPixelFormat,
        plane_index: usize,
    ) -> Result<Retained<Objc2MetalTexture>> {
        let (width, height) = if self.plane_count() == 0 {
            (self.width as usize, self.height as usize)
        } else {
            (
                CVPixelBufferGetWidthOfPlane(&self.pixel_buffer, plane_index),
                CVPixelBufferGetHeightOfPlane(&self.pixel_buffer, plane_index),
            )
        };
        let mut cv_texture: *mut CVMetalTexture = std::ptr::null_mut();
        let status = unsafe {
            CVMetalTextureCache::create_texture_from_image(
                None,
                &cache.inner,
                &self.pixel_buffer,
                None,
                pixel_format,
                width,
                height,
                plane_index,
                NonNull::new_unchecked(&mut cv_texture),
            )
        };
        if status != kCVReturnSuccess {
            return Err(FfmpegError::new(
                "CVMetalTextureCacheCreateTextureFromImage",
                format!("CoreVideo returned {status}"),
            )
            .with_backend(GpuBackend::Metal));
        }
        let cv_texture = NonNull::new(cv_texture).ok_or_else(|| {
            FfmpegError::new(
                "CVMetalTextureCacheCreateTextureFromImage",
                "CoreVideo returned a null texture",
            )
            .with_backend(GpuBackend::Metal)
        })?;
        let cv_texture = unsafe { CFRetained::from_raw(cv_texture) };
        CVMetalTextureGetTexture(&cv_texture).ok_or_else(|| {
            FfmpegError::new(
                "CVMetalTextureGetTexture",
                "CoreVideo returned a null MTLTexture",
            )
            .with_backend(GpuBackend::Metal)
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "vulkan")]
    #[test]
    fn ash_vulkan_frame_reports_shape() {
        use ash::vk::Handle;

        use super::*;

        let frame = GpuVideoInput::Vulkan {
            image: ash::vk::Image::from_raw(11),
            image_view: ash::vk::ImageView::from_raw(22),
            memory: ash::vk::DeviceMemory::from_raw(33),
            format: ash::vk::Format::R8G8B8A8_UNORM,
            extent: ash::vk::Extent3D {
                width: 640,
                height: 480,
                depth: 1,
            },
        };

        assert_eq!(frame.backend(), GpuBackend::Vulkan);
        assert_eq!(frame.dimensions(), (640, 480));
        if let GpuVideoInput::Vulkan {
            image,
            image_view,
            memory,
            format,
            ..
        } = frame
        {
            assert_eq!(image.as_raw(), 11);
            assert_eq!(image_view.as_raw(), 22);
            assert_eq!(memory.as_raw(), 33);
            assert_eq!(format, ash::vk::Format::R8G8B8A8_UNORM);
        }
    }
}
