//! A model's images: PNG or JPEG, and with the crate's features WebP and KTX2, sized from their
//! headers and checked against the limits before any pixel is decoded, with the mip levels a KTX2
//! image lacks, and every level of the others, made here.

use std::borrow::Cow;
use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits};

use super::{
    COMPRESSED_IMAGE_PIXELS_PER_BYTE, MAX_IMAGE_DECODE_BYTES, MAX_IMAGE_SIDE,
    MIN_COMPRESSED_IMAGE_PIXELS, ModelError, Texture, guarded, invalid, too_large, unsupported,
};
use crate::srgb_to_linear;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// The chunks a PNG's still image needs. The rest is left out before decoding, so none of it is
/// ever inflated: colour profiles and gamma, which glTF says to ignore, text, and the frames of an
/// animated PNG, of which only the still image is read.
const PNG_PIXEL_CHUNKS: [[u8; 4]; 5] = [*b"IHDR", *b"PLTE", *b"tRNS", *b"IDAT", *b"IEND"];

/// The formats images may be in.
const FORMATS: &str = match (cfg!(feature = "webp"), cfg!(feature = "ktx2")) {
    (false, false) => "neither PNG nor JPEG",
    (true, false) => "not PNG, JPEG or WebP",
    (false, true) => "not PNG, JPEG or KTX2",
    (true, true) => "not PNG, JPEG, WebP or KTX2",
};

#[derive(Clone, Copy)]
enum Format {
    /// Decoded by the `image` crate.
    Raster(ImageFormat),
    #[cfg(feature = "ktx2")]
    Ktx2(super::ktx2::Container),
}

/// The header of an image, whose size is within [`MAX_IMAGE_SIDE`].
#[derive(Clone, Copy)]
pub(super) struct Header {
    format: Format,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// The image the `image` crate decodes in `bytes` and the bytes its decoder reads: for a PNG,
/// only its [`PNG_PIXEL_CHUNKS`], copied.
fn pixel_bytes(bytes: &[u8]) -> Result<(ImageFormat, Cow<'_, [u8]>), ModelError> {
    match image::guess_format(bytes) {
        Ok(ImageFormat::Png) => Ok((ImageFormat::Png, Cow::Owned(png_pixel_chunks(bytes)?))),
        Ok(ImageFormat::Jpeg) => Ok((ImageFormat::Jpeg, Cow::Borrowed(bytes))),
        #[cfg(feature = "webp")]
        Ok(ImageFormat::WebP) => Ok((ImageFormat::WebP, Cow::Borrowed(bytes))),
        _ => Err(unsupported(format!("an image in the model is {FORMATS}"))),
    }
}

/// Fails an image of `bytes` encoded bytes whose `width` x `height` pixels, with their mips, are
/// more than [`COMPRESSED_IMAGE_PIXELS_PER_BYTE`] allows.
fn check_expansion(width: u32, height: u32, bytes: usize) -> Result<(), ModelError> {
    let allowed = (bytes as u64)
        .saturating_mul(COMPRESSED_IMAGE_PIXELS_PER_BYTE)
        .max(MIN_COMPRESSED_IMAGE_PIXELS);
    if mip_pixels(width, height) > allowed {
        return Err(ModelError::TooLarge(format!(
            "an image in the model decodes to more than the {allowed} pixels its {bytes} bytes may"
        )));
    }
    Ok(())
}

/// Reads the header of the image in `bytes`, keeping nothing of it. A KTX2 image's container is
/// checked whole.
pub(super) fn header(bytes: &[u8]) -> Result<Header, ModelError> {
    #[cfg(feature = "ktx2")]
    if bytes.starts_with(&super::ktx2::MAGIC) {
        let container = super::ktx2::check(bytes)?;
        check_expansion(container.width, container.height, bytes.len())?;
        return Ok(Header {
            format: Format::Ktx2(container),
            width: container.width,
            height: container.height,
        });
    }
    let encoded = bytes.len();
    let (format, bytes) = pixel_bytes(bytes)?;
    // A JPEG's header segments are copied as they are, without decompressing anything, so
    // reading them takes memory and time in proportion to the file.
    let (width, height) = guarded("an image", || {
        ImageReader::with_format(Cursor::new(&*bytes), format)
            .into_dimensions()
            .map_err(corrupt)
    })?;
    if width == 0 || height == 0 {
        return Err(invalid("an image in the model has no pixels"));
    }
    if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        return Err(too_large("pixels across one image", MAX_IMAGE_SIDE));
    }
    if format != ImageFormat::Png && format != ImageFormat::Jpeg {
        check_expansion(width, height, encoded)?;
    }
    Ok(Header {
        format: Format::Raster(format),
        width,
        height,
    })
}

/// The signature of the PNG in `bytes`, followed by those of its chunks its still image needs.
fn png_pixel_chunks(bytes: &[u8]) -> Result<Vec<u8>, ModelError> {
    let truncated = || invalid("an image in the model is corrupt: a chunk reaches past its end");
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&PNG_SIGNATURE);
    let mut rest = bytes.get(PNG_SIGNATURE.len()..).ok_or_else(truncated)?;
    while let Some(header) = rest.get(..8) {
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = [header[4], header[5], header[6], header[7]];
        // The chunk's length and type, data and checksum.
        let end = usize::try_from(length)
            .ok()
            .and_then(|length| length.checked_add(12))
            .ok_or_else(truncated)?;
        let chunk = rest.get(..end).ok_or_else(truncated)?;
        if PNG_PIXEL_CHUNKS.contains(&kind) {
            out.extend_from_slice(chunk);
        }
        if kind == *b"IEND" {
            break;
        }
        rest = &rest[end..];
    }
    Ok(out)
}

fn corrupt(error: ImageError) -> ModelError {
    invalid(format!("an image in the model is corrupt: {error}"))
}

/// Decodes the image in `bytes`, whose header is `header`, to RGBA8 rows, top first, of its full
/// size and, for a KTX2 image, of the mip levels it holds. What the `image` crate's decoder writes,
/// in its own pixel format, is checked against [`MAX_IMAGE_DECODE_BYTES`] from the header's size
/// before anything is decoded. The decoder's own limits are set as a second line, though on this
/// path they don't bound every allocation it makes.
pub(super) fn decode(bytes: &[u8], header: Header) -> Result<Vec<Vec<u8>>, ModelError> {
    let expected = match header.format {
        Format::Raster(format) => format,
        #[cfg(feature = "ktx2")]
        Format::Ktx2(container) => return super::ktx2::decode(bytes, container),
    };
    let (format, bytes) = pixel_bytes(bytes)?;
    if format != expected {
        return Err(invalid(
            "an image in the model changed format between reads",
        ));
    }
    guarded("an image", || {
        let mut reader = ImageReader::with_format(Cursor::new(&*bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_IMAGE_SIDE);
        limits.max_image_height = Some(MAX_IMAGE_SIDE);
        limits.max_alloc = Some(MAX_IMAGE_DECODE_BYTES);
        reader.limits(limits);
        let decoder = reader.into_decoder().map_err(corrupt)?;
        if decoder.dimensions() != (header.width, header.height) {
            return Err(invalid("an image in the model changed size between reads"));
        }
        let native = u64::from(decoder.color_type().bytes_per_pixel())
            * u64::from(header.width)
            * u64::from(header.height);
        if native > MAX_IMAGE_DECODE_BYTES {
            return Err(too_large(
                "bytes in one decoded image",
                MAX_IMAGE_DECODE_BYTES,
            ));
        }
        Ok(vec![
            DynamicImage::from_decoder(decoder)
                .map_err(corrupt)?
                .into_rgba8()
                .into_raw(),
        ])
    })
}

/// The sides of each mip level of a `width` x `height` image, from the full size down to 1 x 1.
fn mip_sizes(width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
    std::iter::successors(Some((width, height)), |&(width, height)| {
        (width > 1 || height > 1).then(|| ((width / 2).max(1), (height / 2).max(1)))
    })
}

/// The pixels of every mip level of a `width` x `height` image together.
pub(super) fn mip_pixels(width: u32, height: u32) -> u64 {
    mip_sizes(width, height)
        .map(|(width, height)| u64::from(width) * u64::from(height))
        .sum()
}

/// `levels`, the first mip levels of a `width` x `height` image as [`decode`] gives them, as a
/// texture with every level, those it lacks averaged in linear light when `srgb`.
pub(super) fn texture(mut levels: Vec<Vec<u8>>, width: u32, height: u32, srgb: bool) -> Texture {
    let sizes: Vec<(u32, u32)> = mip_sizes(width, height).collect();
    let mut from = sizes
        .get(levels.len().saturating_sub(1))
        .copied()
        .unwrap_or((width, height));
    for &to in sizes.iter().skip(levels.len()) {
        let Some(last) = levels.last() else { break };
        let next = downsample(last, from, to, srgb);
        levels.push(next);
        from = to;
    }
    Texture {
        width,
        height,
        srgb,
        levels,
    }
}

/// For each texel along a side `to` long, the texels of a side `from` long it covers, with the
/// share of it each covers, so that every source texel counts in full.
fn footprint(from: u32, to: u32) -> Vec<Vec<(usize, f32)>> {
    let ratio = f64::from(from) / f64::from(to);
    (0..to)
        .map(|texel| {
            let start = f64::from(texel) * ratio;
            let end = start + ratio;
            (start.floor() as usize..(end.ceil() as usize).min(from as usize))
                .map(|source| {
                    let covered = end.min(source as f64 + 1.0) - start.max(source as f64);
                    (source, (covered / ratio) as f32)
                })
                .filter(|&(_, share)| share > 0.0)
                .collect()
        })
        .collect()
}

/// Shrinks an RGBA8 level from `from` to `to` texels a side with a box filter over the whole of
/// it, so that an odd side's last row or column counts too.
pub(super) fn downsample(rgba: &[u8], from: (u32, u32), to: (u32, u32), srgb: bool) -> Vec<u8> {
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
    let (columns, rows) = (footprint(from.0, to.0), footprint(from.1, to.1));
    let width = from.0 as usize;
    let mut out = Vec::with_capacity(to.0 as usize * to.1 as usize * 4);
    for row in &rows {
        for column in &columns {
            let mut sum = [0.0f32; 4];
            for &(y, share_y) in row {
                for &(x, share_x) in column {
                    let share = share_x * share_y;
                    let texel = &rgba[(y * width + x) * 4..][..4];
                    for (total, &value) in sum[..3].iter_mut().zip(&texel[..3]) {
                        *total += linear[usize::from(value)] * share;
                    }
                    sum[3] += f32::from(texel[3]) / 255.0 * share;
                }
            }
            for &total in &sum[..3] {
                out.push(encode(total));
            }
            out.push((sum[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
    }
    out
}
