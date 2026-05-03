use crate::{
    composition::Composition,
    error::RenderError,
    gpu::{BoundFrame, CompileContext, CompiledComposition, FrameBindContext, RasterHandle},
    media::MediaStore,
};

pub struct GpuCompositionRenderer {
    renderer: lumen_gpu::Renderer,
    compiled: Option<CompiledComposition>,
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
        })
    }

    pub fn from_device(device: lumen_gpu::wgpu::Device, queue: lumen_gpu::wgpu::Queue) -> Self {
        Self {
            renderer: lumen_gpu::Renderer::from_device(device, queue),
            compiled: None,
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
