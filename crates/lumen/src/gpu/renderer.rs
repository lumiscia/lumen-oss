use crate::{
    composition::Composition,
    error::RenderError,
    gpu::{CompileContext, CompiledComposition, FrameBindContext, RasterHandle},
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
        let compiled = CompileContext::new(composition).compile()?;
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
        let compiled = self.compiled.as_ref().ok_or_else(|| RenderError::Gpu {
            details: "composition has not been compiled".to_string(),
        })?;
        let bound = FrameBindContext::with_media(composition, frame, media).bind(compiled)?;
        let update = bound.frame_update();
        self.renderer
            .execute(&compiled.plan, &update)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        Ok(compiled.output)
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
