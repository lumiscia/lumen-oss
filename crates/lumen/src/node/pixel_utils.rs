//! Shared pixel utilities used across node implementations.

use std::sync::Arc;

use skia_safe::{AlphaType, ColorType, Data, ImageInfo, images, surfaces};

use crate::{
    raster::{AlphaMode, BitmapFrame, RasterFrame},
    render::RenderContext,
};
pub fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
}

pub fn into_bitmap_parts(raster: RasterFrame) -> (Arc<Vec<u8>>, u32, u32) {
    match raster {
        RasterFrame::Bitmap(frame) => (frame.pixels, frame.storage_width, frame.storage_height),
        RasterFrame::Surface(mut surface_frame) => {
            let width = surface_frame.surface.width();
            let height = surface_frame.surface.height();
            let bytes = match surface_frame.surface.surface_mut() {
                Some(surface) => read_surface_rgba(surface, width, height, None),
                None => rgba_byte_len(width, height)
                    .map(|len| vec![0u8; len])
                    .unwrap_or_default(),
            };
            (Arc::new(bytes), width, height)
        }
    }
}

pub fn read_surface_rgba(
    surface: &mut skia_safe::Surface,
    width: u32,
    height: u32,
    mut ctx: Option<&mut RenderContext>,
) -> Vec<u8> {
    let byte_len = rgba_byte_len(width, height).unwrap_or(4);
    let mut bytes = vec![0_u8; byte_len];
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let output = if surface.read_pixels(&info, bytes.as_mut_slice(), (width * 4) as usize, (0, 0)) {
        bytes
    } else {
        vec![0_u8; byte_len]
    };
    if let Some(ctx) = ctx.as_deref_mut() {
        ctx.record_pixel_allocation_bytes(output.len());
    }
    output
}

pub fn make_skia_image_frame(frame: &BitmapFrame) -> Option<skia_safe::Image> {
    make_skia_image(
        frame.pixels.as_slice(),
        frame.storage_width,
        frame.storage_height,
        frame.row_bytes,
        frame.alpha_mode,
    )
}

pub fn make_skia_image(
    bytes: &[u8],
    width: u32,
    height: u32,
    row_bytes: usize,
    alpha_mode: AlphaMode,
) -> Option<skia_safe::Image> {
    let expected = rgba_byte_len(width, height)?;
    if bytes.len() < expected || row_bytes < (width as usize).saturating_mul(4) {
        return None;
    }
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        match alpha_mode {
            AlphaMode::Premultiplied => AlphaType::Premul,
            AlphaMode::Unpremultiplied => AlphaType::Unpremul,
        },
        None,
    );
    let data = unsafe { Data::new_bytes(bytes) };
    images::raster_from_data(&info, data, row_bytes)
}

pub fn render_with_skia(
    width: u32,
    height: u32,
    mut ctx: Option<&mut RenderContext>,
    draw: impl FnOnce(&skia_safe::Canvas),
) -> Vec<u8> {
    let Some(mut surface) = surfaces::raster_n32_premul((width as i32, height as i32)) else {
        let fallback = rgba_byte_len(width, height)
            .map(|len| vec![0u8; len])
            .unwrap_or_default();
        if let Some(ctx) = ctx.as_deref_mut() {
            ctx.record_pixel_allocation_bytes(fallback.len());
        }
        return fallback;
    };
    surface.canvas().clear(skia_safe::Color::TRANSPARENT);
    draw(surface.canvas());
    read_surface_rgba(&mut surface, width, height, ctx)
}

pub fn to_skia_color(color: [u8; 4]) -> skia_safe::Color {
    skia_safe::Color::from_argb(color[3], color[0], color[1], color[2])
}
