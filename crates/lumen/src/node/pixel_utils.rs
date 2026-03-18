//! Shared pixel utilities used across node implementations.

use std::sync::Arc;

use skia_safe::{AlphaType, ColorType, Data, ImageInfo, image::CachingHint, images, surfaces};

use crate::{
    media::MediaStore,
    raster::{AlphaMode, BitmapFrame, RasterFrame},
    render::{RenderContext, surface::SurfacePool},
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
            let bytes = read_surface_pixels(surface_frame.surface.surface_mut(), width, height);
            (Arc::new(bytes), width, height)
        }
    }
}

pub fn read_surface_pixels(surface: &mut skia_safe::Surface, width: u32, height: u32) -> Vec<u8> {
    let byte_len = rgba_byte_len(width, height).unwrap_or(4);
    let mut bytes = vec![0_u8; byte_len];
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let snapshot = surface.image_snapshot();
    if snapshot.read_pixels(
        &info,
        bytes.as_mut_slice(),
        (width * 4) as usize,
        (0, 0),
        CachingHint::Disallow,
    ) {
        bytes
    } else {
        vec![0_u8; byte_len]
    }
}

pub fn read_surface_rgba<S: SurfacePool, M: MediaStore>(
    surface: &mut skia_safe::Surface,
    width: u32,
    height: u32,
    _ctx: Option<&mut RenderContext<'_, S, M>>,
) -> Vec<u8> {
    read_surface_pixels(surface, width, height)
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

pub fn render_with_skia<S: SurfacePool, M: MediaStore>(
    width: u32,
    height: u32,
    mut ctx: Option<&mut RenderContext<'_, S, M>>,
    draw: impl FnOnce(&skia_safe::Canvas),
) -> Vec<u8> {
    if let Some(ctx_ref) = ctx.as_deref_mut()
        && let Ok(mut surface_ref) = ctx_ref.renderer.surface_pool.acquire_raster(width, height)
    {
        let surface = surface_ref.surface_mut();
        let canvas = surface.canvas();
        canvas.restore_to_count(1);
        canvas.reset_matrix();
        canvas.clear(skia_safe::Color::TRANSPARENT);
        draw(canvas);
        return read_surface_rgba(surface, width, height, None::<&mut RenderContext<'_, S, M>>);
    }

    let Some(mut surface) = surfaces::raster_n32_premul((width as i32, height as i32)) else {
        return rgba_byte_len(width, height)
            .map(|len| vec![0u8; len])
            .unwrap_or_default();
    };
    let canvas = surface.canvas();
    canvas.clear(skia_safe::Color::TRANSPARENT);
    draw(canvas);
    read_surface_rgba(
        &mut surface,
        width,
        height,
        None::<&mut RenderContext<'_, S, M>>,
    )
}

pub fn to_skia_color(color: [u8; 4]) -> skia_safe::Color {
    skia_safe::Color::from_argb(color[3], color[0], color[1], color[2])
}
