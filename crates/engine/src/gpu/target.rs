use lumen_ffmpeg::{GpuVideoInput, MetalPixelBufferFrame, MetalPixelBufferPool, MetalTextureCache};

use crate::error::RenderError;

pub struct MetalVideoToolboxTargetPool {
    texture_cache: MetalTextureCache,
    pixel_buffer_pool: MetalPixelBufferPool,
    size: lumen_gpu::Size,
    format: lumen_gpu::wgpu::TextureFormat,
}

pub struct MetalVideoToolboxTarget {
    pixel_frame: MetalPixelBufferFrame,
    external: lumen_gpu::ExternalTexture,
}

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

impl MetalVideoToolboxTarget {
    pub fn external_texture(&self) -> lumen_gpu::ExternalTexture {
        self.external.clone()
    }

    pub fn video_input(&self) -> GpuVideoInput<'_> {
        GpuVideoInput::MetalPixelBuffer(&self.pixel_frame)
    }
}

fn external_output_usage() -> lumen_gpu::wgpu::TextureUsages {
    lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT
        | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
        | lumen_gpu::wgpu::TextureUsages::COPY_SRC
}
