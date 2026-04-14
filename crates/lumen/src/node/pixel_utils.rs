//! Shared Skia rendering helpers used across node implementations.

use skia_safe::{AlphaType, ColorType, ImageInfo, image::CachingHint};

use crate::{
    media::MediaStore,
    raster::{AlphaMode, RasterFrame, RectI, rgba_byte_len as raster_rgba_byte_len},
    render::{RenderContext, surface::SurfacePool},
};

pub fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    raster_rgba_byte_len(width, height)
}

pub fn read_surface_into(
    surface: &mut skia_safe::Surface,
    width: u32,
    height: u32,
    dst: &mut [u8],
    row_bytes: usize,
) -> bool {
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let snapshot = surface.image_snapshot();
    snapshot.read_pixels(&info, dst, row_bytes, (0, 0), CachingHint::Disallow)
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
) -> crate::Result<RasterFrame> {
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
            let mut frame = crate::raster::ImageFrame::with_domain(
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

pub use crate::raster::make_skia_image;
