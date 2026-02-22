#[cfg(all(feature = "gpu-metal", target_os = "macos"))]
pub mod metal;
pub mod software;
#[cfg(feature = "gpu-vulkan")]
pub mod vulkan;

use skia_safe::{ColorType, IPoint, ImageInfo};
use thiserror::Error;

use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub pixels_rgba: Vec<u8>,
}

pub trait FrameProvider {
    fn image(&mut self, source_id: &str) -> Result<Option<FrameImage>, RenderError>;
    fn video_frame(
        &mut self,
        source_id: &str,
        frame: u64,
    ) -> Result<Option<FrameImage>, RenderError>;
}

pub trait RenderBackend {
    fn render_frame(
        &mut self,
        renderer_ctx: &mut RendererContext,
        frame_ctx: &FrameContext,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError>;
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("unsupported render operation: {0}")]
    Unsupported(&'static str),
    #[error("missing media source: {0}")]
    MissingSource(String),
    #[error("render backend not initialized")]
    NotInitialized,
    #[error("failed to read surface pixels")]
    PixelReadback,
    #[error("failed to create GPU surface")]
    GpuSurfaceCreation,
}

#[allow(dead_code)]
#[cfg(any(feature = "gpu-metal", feature = "gpu-vulkan"))]
pub(super) enum GpuBackend {
    #[cfg(all(feature = "gpu-metal", target_os = "macos"))]
    Metal(metal::MetalState),
    #[cfg(feature = "gpu-vulkan")]
    Vulkan(vulkan::VulkanState),
}

#[cfg(any(feature = "gpu-metal", feature = "gpu-vulkan"))]
pub(super) struct GpuState {
    pub(super) context: skia_safe::gpu::DirectContext,
    pub(super) _backend: GpuBackend,
}

#[cfg(any(feature = "gpu-metal", feature = "gpu-vulkan"))]
pub(super) fn create_gpu_surface(
    context: &mut skia_safe::gpu::DirectContext,
    width: u32,
    height: u32,
) -> Result<skia_safe::Surface, RenderError> {
    use skia_safe::gpu;

    let info = ImageInfo::new_n32_premul((width as i32, height as i32), None);
    gpu::surfaces::render_target(
        context,
        gpu::Budgeted::Yes,
        &info,
        None,
        gpu::SurfaceOrigin::TopLeft,
        None,
        false,
        None,
    )
    .ok_or(RenderError::GpuSurfaceCreation)
}

pub fn pixel_len(width: u32, height: u32) -> Result<usize, RenderError> {
    let len = (width as u64)
        .saturating_mul(height as u64)
        .saturating_mul(4);

    usize::try_from(len).map_err(|_| RenderError::Unsupported("frame size overflow"))
}

pub(crate) fn read_surface_rgba(
    renderer_ctx: &mut RendererContext,
) -> Result<Vec<u8>, RenderError> {
    let required = pixel_len(renderer_ctx.width, renderer_ctx.height)?;
    let mut pixels = vec![0_u8; required];
    let info = ImageInfo::new(
        (renderer_ctx.width as i32, renderer_ctx.height as i32),
        ColorType::RGBA8888,
        skia_safe::AlphaType::Unpremul,
        None,
    );

    let ok = renderer_ctx.surface.read_pixels(
        &info,
        pixels.as_mut_slice(),
        renderer_ctx.width as usize * 4,
        IPoint::new(0, 0),
    );
    if !ok {
        return Err(RenderError::PixelReadback);
    }

    Ok(pixels)
}
