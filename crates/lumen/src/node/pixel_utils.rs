//! Shared pixel utilities used across node implementations.

use std::sync::Arc;

use skia_safe::{AlphaType, ColorType, Data, ImageInfo, images, surfaces};

use crate::raster::RasterFrame;

pub fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
}

pub fn into_bitmap_parts(raster: RasterFrame) -> (Arc<Vec<u8>>, u32, u32) {
    match raster {
        RasterFrame::Bitmap(bytes, width, height) => (bytes, width, height),
        RasterFrame::Surface(mut surface_ref) => {
            let width = surface_ref.width();
            let height = surface_ref.height();
            let bytes = match surface_ref.surface_mut() {
                Some(surface) => read_surface_rgba(surface, width, height),
                None => rgba_byte_len(width, height)
                    .map(|len| vec![0u8; len])
                    .unwrap_or_default(),
            };
            (Arc::new(bytes), width, height)
        }
    }
}

pub fn read_surface_rgba(surface: &mut skia_safe::Surface, width: u32, height: u32) -> Vec<u8> {
    let byte_len = rgba_byte_len(width, height).unwrap_or(4);
    let mut bytes = vec![0_u8; byte_len];
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    if surface.read_pixels(&info, bytes.as_mut_slice(), (width * 4) as usize, (0, 0)) {
        bytes
    } else {
        vec![0_u8; byte_len]
    }
}

pub fn make_skia_image(bytes: &[u8], width: u32, height: u32) -> Option<skia_safe::Image> {
    let expected = rgba_byte_len(width, height)?;
    if bytes.len() != expected {
        return None;
    }
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let row_bytes = (width * 4) as usize;
    let data = unsafe { Data::new_bytes(bytes) };
    images::raster_from_data(&info, data, row_bytes)
}

pub fn render_with_skia(
    width: u32,
    height: u32,
    draw: impl FnOnce(&skia_safe::Canvas),
) -> Vec<u8> {
    let Some(mut surface) = surfaces::raster_n32_premul((width as i32, height as i32)) else {
        return rgba_byte_len(width, height)
            .map(|len| vec![0u8; len])
            .unwrap_or_default();
    };
    surface.canvas().clear(skia_safe::Color::TRANSPARENT);
    draw(surface.canvas());
    read_surface_rgba(&mut surface, width, height)
}

pub fn to_skia_color(color: [u8; 4]) -> skia_safe::Color {
    skia_safe::Color::from_argb(color[3], color[0], color[1], color[2])
}
