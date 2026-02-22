use crate::render::backend::{FrameProvider, RenderBackend, RenderError};
use crate::render::context::{FrameContext, RendererContext};

use objc2::rc::Retained;
use objc2_metal::{MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice};
use skia_safe::gpu;

use super::{GpuBackend, GpuState, create_gpu_surface};

pub struct MetalRenderBackend {
    gpu: Option<super::GpuState>,
}

impl Default for MetalRenderBackend {
    fn default() -> Self {
        Self { gpu: None }
    }
}

impl RenderBackend for MetalRenderBackend {
    fn render_frame(
        &mut self,
        renderer_ctx: &mut RendererContext,
        _frame_ctx: &FrameContext,
        _provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        self.ensure_gpu_surface(renderer_ctx)?;
        renderer_ctx.clear();
        super::read_surface_rgba(renderer_ctx)
    }
}

impl MetalRenderBackend {
    fn ensure_gpu_surface(
        &mut self,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        if let Some(gpu) = self.gpu.as_mut() {
            renderer_ctx.surface = super::create_gpu_surface(
                &mut gpu.context,
                renderer_ctx.width,
                renderer_ctx.height,
            )?;
            return Ok(());
        }

        let (surface, gpu) = try_create(renderer_ctx.width, renderer_ctx.height)
            .ok_or(RenderError::NotInitialized)?;
        renderer_ctx.surface = surface;
        self.gpu = Some(gpu);
        Ok(())
    }
}

pub(in crate::render) struct MetalState {
    pub _device: Retained<objc2::runtime::ProtocolObject<dyn MTLDevice>>,
    pub _queue: Retained<objc2::runtime::ProtocolObject<dyn MTLCommandQueue>>,
}

pub(super) fn try_create(width: u32, height: u32) -> Option<(skia_safe::Surface, GpuState)> {
    let device = MTLCreateSystemDefaultDevice()?;
    let queue = device.newCommandQueue()?;

    let backend = unsafe {
        gpu::mtl::BackendContext::new(
            Retained::as_ptr(&device) as gpu::mtl::Handle,
            Retained::as_ptr(&queue) as gpu::mtl::Handle,
        )
    };

    let mut context = gpu::direct_contexts::make_metal(&backend, None)?;
    let surface = create_gpu_surface(&mut context, width, height).ok()?;

    Some((
        surface,
        GpuState {
            context,
            _backend: GpuBackend::Metal(MetalState {
                _device: device,
                _queue: queue,
            }),
        },
    ))
}
