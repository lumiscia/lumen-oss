#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use std::sync::OnceLock;

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_ffmpeg::{
    CudaContext, CudaDeviceAllocation, CudaDriver, CudaVideoFrame, GpuVideoInput,
    ImportedCudaExternalImage, import_owned_vulkan_opaque_fd_image,
};
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_ffmpeg::{GpuVideoInput, MetalPixelBufferFrame, MetalPixelBufferPool, MetalTextureCache};

use crate::error::RenderError;

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
pub struct CudaNvencTargetPool {
    driver: &'static CudaDriver,
    cuda_ordinal: i32,
    size: lumen_gpu::Size,
    format: lumen_gpu::wgpu::TextureFormat,
    vulkan_device: lumen_gpu::VulkanDeviceInfo,
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
pub struct CudaNvencTarget {
    driver: &'static CudaDriver,
    external: lumen_gpu::ExternalTexture,
    cuda_frame: CudaDeviceAllocation<'static>,
    imported: ImportedCudaExternalImage,
    exportable: lumen_gpu::ExportableVulkanTexture,
    context: CudaContext<'static>,
}

#[cfg(all(target_os = "macos", feature = "metal"))]
pub struct MetalVideoToolboxTargetPool {
    texture_cache: MetalTextureCache,
    pixel_buffer_pool: MetalPixelBufferPool,
    size: lumen_gpu::Size,
    format: lumen_gpu::wgpu::TextureFormat,
}

#[cfg(all(target_os = "macos", feature = "metal"))]
pub struct MetalVideoToolboxTarget {
    pixel_frame: MetalPixelBufferFrame,
    external: lumen_gpu::ExternalTexture,
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl CudaNvencTargetPool {
    pub fn rgba8(renderer: &lumen_gpu::Renderer, size: lumen_gpu::Size) -> crate::Result<Self> {
        let bootstrap = renderer
            .create_exportable_vulkan_texture(
                Some("lumen CUDA/NVENC target bootstrap"),
                size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                cuda_nvenc_usage(),
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let vulkan_device = bootstrap.device_info().clone();
        let driver = cuda_driver().map_err(|details| RenderError::Gpu { details })?;
        let cuda_ordinal = driver
            .devices()
            .map_err(|details| RenderError::Gpu { details })?
            .into_iter()
            .find(|device| device.uuid == vulkan_device.device_uuid)
            .map(|device| device.ordinal)
            .ok_or_else(|| RenderError::Gpu {
                details: format!(
                    "no CUDA device UUID matched Vulkan device {} ({})",
                    vulkan_device.name,
                    format_uuid(&vulkan_device.device_uuid)
                ),
            })?;
        drop(bootstrap);
        Ok(Self {
            driver,
            cuda_ordinal,
            size,
            format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            vulkan_device,
        })
    }

    pub fn acquire(&self, renderer: &lumen_gpu::Renderer) -> crate::Result<CudaNvencTarget> {
        let context = self
            .driver
            .create_primary_context_for_ordinal(self.cuda_ordinal)
            .map_err(|details| RenderError::Gpu { details })?;
        let exportable = renderer
            .create_exportable_vulkan_texture(
                Some("lumen CUDA/NVENC render target"),
                self.size,
                self.format,
                cuda_nvenc_usage(),
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let imported = import_owned_vulkan_opaque_fd_image(
            self.driver,
            exportable
                .memory_fd()
                .try_clone()
                .map_err(|error| RenderError::Gpu {
                    details: format!("failed to duplicate Vulkan memory fd: {error}"),
                })?,
            exportable.allocation_size(),
            self.size.width,
            self.size.height,
        )
        .map_err(|details| RenderError::Gpu { details })?;
        let cuda_frame = self
            .driver
            .allocate_rgba_frame(self.size.width, self.size.height)
            .map_err(|details| RenderError::Gpu { details })?;
        let external = lumen_gpu::ExternalTexture::new(
            exportable.texture_arc(),
            lumen_gpu::TextureDesc {
                domain: lumen_gpu::TextureDomain::full_frame(self.size),
                format: self.format,
                usage: cuda_nvenc_usage(),
            },
        );
        Ok(CudaNvencTarget {
            driver: self.driver,
            external,
            cuda_frame,
            imported,
            exportable,
            context,
        })
    }

    pub fn format(&self) -> lumen_gpu::wgpu::TextureFormat {
        self.format
    }

    pub fn size(&self) -> lumen_gpu::Size {
        self.size
    }

    pub fn vulkan_device(&self) -> &lumen_gpu::VulkanDeviceInfo {
        &self.vulkan_device
    }

    pub fn cuda_ordinal(&self) -> i32 {
        self.cuda_ordinal
    }

    pub fn driver(&self) -> &'static CudaDriver {
        self.driver
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl CudaNvencTarget {
    pub fn external_texture(&self) -> lumen_gpu::ExternalTexture {
        self.external.clone()
    }

    pub fn copy_rendered_frame_to_cuda(&self) -> crate::Result<()> {
        self.context
            .set_current()
            .map_err(|details| RenderError::Gpu { details })?;
        Ok(self
            .driver
            .copy_image_to_rgba_frame(&self.imported, &self.cuda_frame)
            .map_err(|details| RenderError::Gpu { details })?)
    }

    pub fn video_frame(&self, pts: Option<i64>) -> CudaVideoFrame {
        self.cuda_frame.as_video_frame(pts)
    }

    pub fn video_input<'a>(&'a self, frame: &'a CudaVideoFrame) -> GpuVideoInput<'a> {
        GpuVideoInput::Cuda(frame)
    }

    pub fn allocation_size(&self) -> u64 {
        self.exportable.allocation_size()
    }

    pub fn row_pitch(&self) -> u64 {
        self.exportable.row_pitch()
    }

    pub fn memory_fd_raw(&self) -> i32 {
        self.exportable.memory_fd_raw()
    }

    pub fn memory_type_index(&self) -> u32 {
        self.exportable.memory_type_index()
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl Drop for CudaNvencTarget {
    fn drop(&mut self) {
        let _ = self.context.set_current();
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn cuda_driver() -> Result<&'static CudaDriver, String> {
    static CUDA_DRIVER: OnceLock<CudaDriver> = OnceLock::new();
    if let Some(driver) = CUDA_DRIVER.get() {
        return Ok(driver);
    }
    let _ = CUDA_DRIVER.set(CudaDriver::load()?);
    CUDA_DRIVER
        .get()
        .ok_or_else(|| "CUDA driver was not initialized".to_string())
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn cuda_nvenc_usage() -> lumen_gpu::wgpu::TextureUsages {
    lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT
        | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
        | lumen_gpu::wgpu::TextureUsages::STORAGE_BINDING
        | lumen_gpu::wgpu::TextureUsages::COPY_DST
        | lumen_gpu::wgpu::TextureUsages::COPY_SRC
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn format_uuid(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(all(target_os = "macos", feature = "metal"))]
impl MetalVideoToolboxTargetPool {
    pub fn bgra8(renderer: &lumen_gpu::Renderer, size: lumen_gpu::Size) -> crate::Result<Self> {
        let metal_device =
            lumen_gpu::metal_device_from_wgpu(&renderer.device).map_err(|error| {
                RenderError::Gpu {
                    details: error.to_string(),
                }
            })?;
        let texture_cache =
            MetalTextureCache::create(&metal_device).map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let pixel_buffer_pool =
            MetalPixelBufferPool::bgra8(size.width, size.height).map_err(|error| {
                RenderError::Gpu {
                    details: error.to_string(),
                }
            })?;
        Ok(Self {
            texture_cache,
            pixel_buffer_pool,
            size,
            format: lumen_gpu::wgpu::TextureFormat::Bgra8Unorm,
        })
    }

    pub fn acquire(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        frame: u32,
    ) -> crate::Result<MetalVideoToolboxTarget> {
        let pixel_frame = self
            .pixel_buffer_pool
            .create_frame(Some(i64::from(frame)))
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let metal_texture = pixel_frame
            .create_texture(
                &self.texture_cache,
                objc2_metal::MTLPixelFormat::BGRA8Unorm,
                0,
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let texture = lumen_gpu::texture_from_metal(
            &renderer.device,
            metal_texture,
            &lumen_gpu::wgpu::TextureDescriptor {
                label: Some("lumen VideoToolbox render target"),
                size: self.size.as_extent(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: lumen_gpu::wgpu::TextureDimension::D2,
                format: self.format,
                usage: external_output_usage(),
                view_formats: &[],
            },
        );
        Ok(MetalVideoToolboxTarget {
            pixel_frame,
            external: lumen_gpu::ExternalTexture::from_texture(
                texture,
                lumen_gpu::TextureDesc {
                    domain: lumen_gpu::TextureDomain::full_frame(self.size),
                    format: self.format,
                    usage: external_output_usage(),
                },
            ),
        })
    }

    pub fn format(&self) -> lumen_gpu::wgpu::TextureFormat {
        self.format
    }

    pub fn size(&self) -> lumen_gpu::Size {
        self.size
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
impl MetalVideoToolboxTarget {
    pub fn external_texture(&self) -> lumen_gpu::ExternalTexture {
        self.external.clone()
    }

    pub fn video_input(&self) -> GpuVideoInput<'_> {
        GpuVideoInput::MetalPixelBuffer(&self.pixel_frame)
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn external_output_usage() -> lumen_gpu::wgpu::TextureUsages {
    lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT
        | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
        | lumen_gpu::wgpu::TextureUsages::COPY_SRC
}
