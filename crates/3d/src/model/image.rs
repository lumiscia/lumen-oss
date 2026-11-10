//! A model's images: PNG or JPEG, decoded within the size limits, with mip levels made here.

use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

use super::{
    MAX_IMAGE_SIDE, MAX_TEXTURE_PIXELS, ModelError, Texture, invalid, too_large, unsupported,
};
use crate::srgb_to_linear;

/// The most bytes decoding one image may allocate.
const MAX_DECODE_ALLOC: u64 = 1 << 29;

/// An image's RGBA8 pixels.
pub(super) struct Decoded {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

/// Decodes `bytes` if its pixels fit with the `pixels` textures already hold within
/// [`MAX_TEXTURE_PIXELS`], which is checked before any pixel is decoded.
pub(super) fn decode(bytes: &[u8], pixels: u64) -> Result<Decoded, ModelError> {
    let format = match image::guess_format(bytes) {
        Ok(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => format,
        _ => {
            return Err(unsupported("an image in the model is neither PNG nor JPEG"));
        }
    };
    let corrupt =
        |error: image::ImageError| invalid(format!("an image in the model is corrupt: {error}"));
    // A decoder that panics on a corrupt image fails this image rather than the process, where
    // panics unwind.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = Limits::default();
        limits.max_alloc = Some(MAX_DECODE_ALLOC);
        reader.limits(limits);
        let decoder = reader.into_decoder().map_err(corrupt)?;
        let (width, height) = decoder.dimensions();
        if width == 0 || height == 0 {
            return Err(invalid("an image in the model has no pixels"));
        }
        if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
            return Err(too_large("pixels across one image", MAX_IMAGE_SIDE));
        }
        if pixels.saturating_add(u64::from(width) * u64::from(height)) > MAX_TEXTURE_PIXELS {
            return Err(too_large("texture pixels", MAX_TEXTURE_PIXELS));
        }
        let rgba = DynamicImage::from_decoder(decoder)
            .map_err(corrupt)?
            .into_rgba8();
        Ok(Decoded {
            width,
            height,
            rgba: rgba.into_raw(),
        })
    }))
    .unwrap_or_else(|_| Err(invalid("an image in the model is corrupt")))
}

/// `image` as a texture with every mip level, averaged in linear light when `srgb`.
pub(super) fn texture(image: &Decoded, srgb: bool) -> Texture {
    let mut levels = vec![image.rgba.clone()];
    let (mut width, mut height) = (image.width, image.height);
    while width > 1 || height > 1 {
        let Some(last) = levels.last() else { break };
        let next = downsample(last, width, height, srgb);
        width = (width / 2).max(1);
        height = (height / 2).max(1);
        levels.push(next);
    }
    Texture {
        width: image.width,
        height: image.height,
        srgb,
        levels,
    }
}

/// Halves an RGBA8 level, averaging each 2 x 2 block, or the edge texels of an odd side.
fn downsample(rgba: &[u8], width: u32, height: u32, srgb: bool) -> Vec<u8> {
    let linear: [f32; 256] = std::array::from_fn(|value| {
        if srgb {
            srgb_to_linear(value as u8)
        } else {
            value as f32 / 255.0
        }
    });
    let encode = |c: f32| {
        let c = if !srgb {
            c
        } else if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    };
    let (next_width, next_height) = ((width / 2).max(1), (height / 2).max(1));
    let (width, height) = (width as usize, height as usize);
    let mut out = Vec::with_capacity(next_width as usize * next_height as usize * 4);
    for y in 0..next_height as usize {
        for x in 0..next_width as usize {
            let mut sum = [0.0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (x * 2 + dx).min(width - 1);
                let sy = (y * 2 + dy).min(height - 1);
                let texel = &rgba[(sy * width + sx) * 4..][..4];
                for channel in 0..3 {
                    sum[channel] += linear[usize::from(texel[channel])];
                }
                sum[3] += f32::from(texel[3]) / 255.0;
            }
            for total in &sum[..3] {
                out.push(encode(total / 4.0));
            }
            out.push((sum[3] / 4.0 * 255.0 + 0.5) as u8);
        }
    }
    out
}
