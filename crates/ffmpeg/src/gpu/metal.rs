use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFType};
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane,
    CVPixelBufferPool, kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_32BGRA, kCVPixelFormatType_32RGBA, kCVReturnSuccess,
};

use crate::{FfmpegError, Result, gpu::GpuBackend};

pub type Objc2MetalTexture = objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>;
pub type Objc2MetalDevice = objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>;

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
pub struct MetalPixelBufferPool {
    inner: CFRetained<CVPixelBufferPool>,
    width: u32,
    height: u32,
    pixel_format: u32,
}

#[cfg(feature = "metal")]
impl std::fmt::Debug for MetalPixelBufferPool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalPixelBufferPool")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixel_format", &self.pixel_format)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "metal")]
impl MetalPixelBufferPool {
    pub fn rgba8(width: u32, height: u32) -> Result<Self> {
        Self::create(width, height, kCVPixelFormatType_32RGBA)
    }

    pub fn bgra8(width: u32, height: u32) -> Result<Self> {
        Self::create(width, height, kCVPixelFormatType_32BGRA)
    }

    pub fn create(width: u32, height: u32, pixel_format: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(FfmpegError::new(
                "CVPixelBufferPoolCreate",
                "width and height must be greater than zero",
            )
            .with_backend(GpuBackend::Metal));
        }

        let width_value = CFNumber::new_i32(width.min(i32::MAX as u32) as i32);
        let height_value = CFNumber::new_i32(height.min(i32::MAX as u32) as i32);
        let pixel_format_value = CFNumber::new_i32(pixel_format as i32);
        let metal_compatible = CFBoolean::new(true);
        let io_surface_properties = CFDictionary::<CFType, CFType>::empty();
        let keys: [&CFType; 5] = unsafe {
            [
                kCVPixelBufferWidthKey.as_ref(),
                kCVPixelBufferHeightKey.as_ref(),
                kCVPixelBufferPixelFormatTypeKey.as_ref(),
                kCVPixelBufferMetalCompatibilityKey.as_ref(),
                kCVPixelBufferIOSurfacePropertiesKey.as_ref(),
            ]
        };
        let values: [&CFType; 5] = [
            width_value.as_ref(),
            height_value.as_ref(),
            pixel_format_value.as_ref(),
            metal_compatible.as_ref(),
            io_surface_properties.as_ref(),
        ];
        let attributes = CFDictionary::<CFType, CFType>::from_slices(&keys, &values);

        let mut pool: *mut CVPixelBufferPool = std::ptr::null_mut();
        let status = unsafe {
            CVPixelBufferPool::create(
                None,
                None,
                Some(attributes.as_ref()),
                NonNull::new_unchecked(&mut pool),
            )
        };
        if status != kCVReturnSuccess {
            return Err(FfmpegError::new(
                "CVPixelBufferPoolCreate",
                format!("CoreVideo returned {status}"),
            )
            .with_backend(GpuBackend::Metal));
        }
        let pool = NonNull::new(pool).ok_or_else(|| {
            FfmpegError::new("CVPixelBufferPoolCreate", "CoreVideo returned a null pool")
                .with_backend(GpuBackend::Metal)
        })?;
        Ok(Self {
            inner: unsafe { CFRetained::from_raw(pool) },
            width,
            height,
            pixel_format,
        })
    }

    pub fn create_frame(&self, pts: Option<i64>) -> Result<MetalPixelBufferFrame> {
        let mut pixel_buffer: *mut CVPixelBuffer = std::ptr::null_mut();
        let status = unsafe {
            CVPixelBufferPool::create_pixel_buffer(
                None,
                &self.inner,
                NonNull::new_unchecked(&mut pixel_buffer),
            )
        };
        if status != kCVReturnSuccess {
            return Err(FfmpegError::new(
                "CVPixelBufferPoolCreatePixelBuffer",
                format!("CoreVideo returned {status}"),
            )
            .with_backend(GpuBackend::Metal));
        }
        let pixel_buffer = NonNull::new(pixel_buffer).ok_or_else(|| {
            FfmpegError::new(
                "CVPixelBufferPoolCreatePixelBuffer",
                "CoreVideo returned a null pixel buffer",
            )
            .with_backend(GpuBackend::Metal)
        })?;
        Ok(unsafe { MetalPixelBufferFrame::from_retained_pixel_buffer(pixel_buffer, pts) })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pixel_format(&self) -> u32 {
        self.pixel_format
    }
}

#[cfg(feature = "metal")]
pub struct MetalPixelBufferFrame {
    pixel_buffer: CFRetained<CVPixelBuffer>,
    width: u32,
    height: u32,
    pixel_format: u32,
    pts: Option<i64>,
}

#[cfg(feature = "metal")]
impl std::fmt::Debug for MetalPixelBufferFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalPixelBufferFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixel_format", &self.pixel_format)
            .field("pts", &self.pts)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "metal")]
impl MetalPixelBufferFrame {
    /// Retains a Core Video pixel buffer for encoding.
    ///
    /// # Safety
    ///
    /// `pixel_buffer` must be a valid `CVPixelBuffer` pointer.
    pub unsafe fn retain_pixel_buffer(
        pixel_buffer: NonNull<CVPixelBuffer>,
        pts: Option<i64>,
    ) -> Self {
        let pixel_buffer = unsafe { CFRetained::retain(pixel_buffer) };
        Self::from_pixel_buffer(pixel_buffer, pts)
    }

    unsafe fn from_retained_pixel_buffer(
        pixel_buffer: NonNull<CVPixelBuffer>,
        pts: Option<i64>,
    ) -> Self {
        let pixel_buffer = unsafe { CFRetained::from_raw(pixel_buffer) };
        Self::from_pixel_buffer(pixel_buffer, pts)
    }

    fn from_pixel_buffer(pixel_buffer: CFRetained<CVPixelBuffer>, pts: Option<i64>) -> Self {
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

    pub fn pixel_buffer_ptr(&self) -> NonNull<CVPixelBuffer> {
        CFRetained::as_ptr(&self.pixel_buffer)
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
        create_metal_texture_from_pixel_buffer(
            &self.pixel_buffer,
            self.width,
            self.height,
            self.plane_count(),
            cache,
            pixel_format,
            plane_index,
        )
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
        create_metal_texture_from_pixel_buffer(
            &self.pixel_buffer,
            self.width,
            self.height,
            self.plane_count(),
            cache,
            pixel_format,
            plane_index,
        )
    }
}

#[cfg(feature = "metal")]
fn create_metal_texture_from_pixel_buffer(
    pixel_buffer: &CVPixelBuffer,
    width: u32,
    height: u32,
    plane_count: usize,
    cache: &MetalTextureCache,
    pixel_format: objc2_metal::MTLPixelFormat,
    plane_index: usize,
) -> Result<Retained<Objc2MetalTexture>> {
    let (width, height) = if plane_count == 0 {
        (width as usize, height as usize)
    } else {
        (
            CVPixelBufferGetWidthOfPlane(pixel_buffer, plane_index),
            CVPixelBufferGetHeightOfPlane(pixel_buffer, plane_index),
        )
    };
    let mut cv_texture: *mut CVMetalTexture = std::ptr::null_mut();
    let status = unsafe {
        CVMetalTextureCache::create_texture_from_image(
            None,
            &cache.inner,
            pixel_buffer,
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
