use std::{collections::HashMap, sync::Arc};

use crate::{
    composition::Composition,
    error::RenderError,
    gpu::{
        BoundFrame, CompileContext, CompiledComposition, FrameBindContext, MediaTextureKey,
        RasterHandle,
    },
    media::{CpuMediaFrame, MediaStore},
};

pub struct GpuCompositionRenderer {
    renderer: lumen_gpu::Renderer,
    compiled: Option<CompiledComposition>,
    media_texture_cache: HashMap<MediaTextureKey, Arc<lumen_gpu::wgpu::Texture>>,
    current_media_textures: HashMap<lumen_gpu::TextureId, MediaTextureKey>,
}

impl GpuCompositionRenderer {
    pub async fn new() -> crate::Result<Self> {
        let renderer = lumen_gpu::Renderer::new()
            .await
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        Ok(Self {
            renderer,
            compiled: None,
            media_texture_cache: HashMap::new(),
            current_media_textures: HashMap::new(),
        })
    }

    pub fn from_device(device: lumen_gpu::wgpu::Device, queue: lumen_gpu::wgpu::Queue) -> Self {
        Self {
            renderer: lumen_gpu::Renderer::from_device(device, queue),
            compiled: None,
            media_texture_cache: HashMap::new(),
            current_media_textures: HashMap::new(),
        }
    }

    pub fn compile(&mut self, composition: &Composition) -> crate::Result<()> {
        self.compile_with_output_format(composition, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
    }

    pub fn compile_with_output_format(
        &mut self,
        composition: &Composition,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> crate::Result<()> {
        let compiled = CompileContext::with_output_format(composition, output_format).compile()?;
        self.prepare_compiled(compiled)
    }

    pub fn compile_with_media<M: MediaStore>(
        &mut self,
        composition: &Composition,
        media: &M,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> crate::Result<()> {
        let compiled = CompileContext::with_media(composition, media, output_format).compile()?;
        self.prepare_compiled(compiled)
    }

    fn prepare_compiled(&mut self, compiled: CompiledComposition) -> crate::Result<()> {
        self.renderer
            .prepare_plan(&compiled.plan)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        self.compiled = Some(compiled);
        Ok(())
    }

    pub fn render_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<RasterHandle> {
        self.render_frame_submitted(composition, frame, media)
            .map(|(raster, _submission)| raster)
    }

    pub fn render_frame_submitted<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        let bound = self.bind_frame(composition, frame, media)?;
        self.submit_bound_frame(&bound)
    }

    pub fn bind_frame<M: MediaStore>(
        &self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<BoundFrame> {
        let compiled = self.compiled.as_ref().ok_or_else(|| RenderError::Gpu {
            details: "composition has not been compiled".to_string(),
        })?;
        FrameBindContext::with_media(composition, frame, media).bind(compiled)
    }

    pub fn submit_bound_frame(
        &mut self,
        bound: &BoundFrame,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        self.upload_bound_frame(bound)?;
        self.submit_render()
    }

    pub fn upload_bound_frame(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        self.upload_media_textures(bound)?;
        let compiled = self.compiled.as_ref().ok_or_else(|| RenderError::Gpu {
            details: "composition has not been compiled".to_string(),
        })?;
        let update = bound.frame_update();
        self.renderer
            .apply_frame_update(&compiled.plan, &update)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        Ok(())
    }

    fn upload_media_textures(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        for upload in bound.media_textures() {
            if upload.key.frame.is_some() {
                let rgba = fit_frame_to_rgba8(&upload.frame, upload.size.width, upload.size.height);
                self.renderer.queue.write_texture(
                    lumen_gpu::wgpu::TexelCopyTextureInfo {
                        texture: self.renderer.texture(upload.texture).ok_or_else(|| {
                            RenderError::Gpu {
                                details: format!("unknown media texture {:?}", upload.texture),
                            }
                        })?,
                        mip_level: 0,
                        origin: lumen_gpu::wgpu::Origin3d::ZERO,
                        aspect: lumen_gpu::wgpu::TextureAspect::All,
                    },
                    &rgba,
                    lumen_gpu::wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(upload.size.width * 4),
                        rows_per_image: Some(upload.size.height),
                    },
                    upload.size.as_extent(),
                );
                self.current_media_textures.remove(&upload.texture);
                continue;
            }

            if self
                .current_media_textures
                .get(&upload.texture)
                .is_some_and(|current| current == &upload.key)
            {
                continue;
            }

            let texture = if let Some(texture) = self.media_texture_cache.get(&upload.key) {
                Arc::clone(texture)
            } else {
                let texture = Arc::new(self.renderer.device.create_texture(
                    &lumen_gpu::wgpu::TextureDescriptor {
                        label: Some("lumen media cached frame"),
                        size: upload.size.as_extent(),
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: lumen_gpu::wgpu::TextureDimension::D2,
                        format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        usage: lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
                            | lumen_gpu::wgpu::TextureUsages::COPY_DST
                            | lumen_gpu::wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    },
                ));
                let rgba = fit_frame_to_rgba8(&upload.frame, upload.size.width, upload.size.height);
                self.renderer.queue.write_texture(
                    texture.as_image_copy(),
                    &rgba,
                    lumen_gpu::wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(upload.size.width * 4),
                        rows_per_image: Some(upload.size.height),
                    },
                    upload.size.as_extent(),
                );
                self.media_texture_cache
                    .insert(upload.key.clone(), Arc::clone(&texture));
                texture
            };

            let desc = lumen_gpu::TextureDesc::sampled(
                upload.size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            );
            self.renderer
                .replace_texture_arc(upload.texture, texture, desc)
                .map_err(|error| RenderError::Gpu {
                    details: error.to_string(),
                })?;
            self.current_media_textures
                .insert(upload.texture, upload.key.clone());
        }
        Ok(())
    }

    pub fn submit_render(
        &mut self,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        let compiled = self.compiled.as_ref().ok_or_else(|| RenderError::Gpu {
            details: "composition has not been compiled".to_string(),
        })?;
        let submission =
            self.renderer
                .submit_plan(&compiled.plan)
                .map_err(|error| RenderError::Gpu {
                    details: error.to_string(),
                })?;
        Ok((compiled.output, submission))
    }

    pub fn gpu_renderer(&self) -> &lumen_gpu::Renderer {
        &self.renderer
    }

    pub fn gpu_renderer_mut(&mut self) -> &mut lumen_gpu::Renderer {
        &mut self.renderer
    }

    pub fn compiled(&self) -> Option<&CompiledComposition> {
        self.compiled.as_ref()
    }
}

fn fit_frame_to_rgba8(frame: &CpuMediaFrame, width: u32, height: u32) -> Vec<u8> {
    if frame.width == width && frame.height == height && frame.row_bytes == width as usize * 4 {
        return frame.rgba.as_ref().clone();
    }

    let mut out = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        let src_y = ((u64::from(y) * u64::from(frame.height)) / u64::from(height)) as usize;
        for x in 0..width {
            let src_x = ((u64::from(x) * u64::from(frame.width)) / u64::from(width)) as usize;
            let src = src_y
                .saturating_mul(frame.row_bytes)
                .saturating_add(src_x.saturating_mul(4));
            let dst = (y as usize)
                .saturating_mul(width as usize * 4)
                .saturating_add(x as usize * 4);
            out[dst..dst + 4].copy_from_slice(&frame.rgba[src..src + 4]);
        }
    }
    out
}
