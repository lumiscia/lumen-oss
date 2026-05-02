//! Shared Skia rendering helpers used across node implementations.

use crate::{
    gpu_image::{AlphaMode, GpuImageFrame, RectI, rgba_byte_len as raster_rgba_byte_len},
    media::MediaStore,
    render::{RenderContext, surface::SurfacePool},
};

pub fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    raster_rgba_byte_len(width, height)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearMode {
    Transparent,
    None,
}

pub fn render_to_surface_ephemeral<S: SurfacePool, M: MediaStore>(
    width: u32,
    height: u32,
    ctx: &mut RenderContext<'_, S, M>,
    format_rect: RectI,
    data_rect: RectI,
    alpha_mode: AlphaMode,
    clear_mode: ClearMode,
    draw: impl FnOnce(&skia_safe::Canvas),
) -> crate::Result<GpuImageFrame> {
    ctx.renderer
        .surface_pool
        .with_surface(width, height, |surface| {
            {
                let canvas = surface.canvas();
                canvas.restore_to_count(1);
                canvas.reset_matrix();
                if clear_mode == ClearMode::Transparent {
                    canvas.clear(skia_safe::Color::TRANSPARENT);
                }
                draw(canvas);
            }
            let image = surface.image_snapshot();
            let mut frame = crate::gpu_image::GpuImageFrame::with_domain(
                image,
                width,
                height,
                format_rect,
                data_rect,
            );
            frame.alpha_mode = alpha_mode;
            Ok(frame)
        })
}

pub fn to_skia_color(color: [u8; 4]) -> skia_safe::Color {
    skia_safe::Color::from_argb(color[3], color[0], color[1], color[2])
}
