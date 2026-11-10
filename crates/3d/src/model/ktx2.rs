//! `KHR_texture_basisu`: KTX2 images of Basis Universal ETC1S or UASTC texels, transcoded to RGBA8
//! by the `basisu` crate, one mip level at a time.
//!
//! The container is checked here before the crate reads any of it, since the crate trusts its
//! offsets: a 2D image within [`MAX_IMAGE_SIDE`] a side, no more levels than its mip chain has,
//! every offset and length within the image, each UASTC level exactly as long as its blocks need,
//! and the ETC1S codebooks and slices within their sections. What the crate then allocates
//! follows from those sizes, and each level it transcodes is checked to be the size it should.

use super::{MAX_IMAGE_SIDE, ModelError, guarded, invalid, too_large, unsupported};

pub(super) const MAGIC: [u8; 12] = [
    0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n',
];

const ETC1S: u8 = 163;
const UASTC: u8 = 166;
const NO_SUPERCOMPRESSION: u32 = 0;
const BASIS_LZ: u32 = 1;
const ZSTANDARD: u32 = 2;
/// The level index's entries, and each ETC1S image's slice descriptor, in bytes.
const LEVEL_ENTRY: u64 = 24;
const IMAGE_DESCRIPTOR: u64 = 20;

/// A checked KTX2 image's size and how many mip levels it holds, from the full size down.
#[derive(Clone, Copy)]
pub(super) struct Container {
    pub(super) width: u32,
    pub(super) height: u32,
    levels: u32,
}

fn u32_at(bytes: &[u8], at: u64) -> Option<u32> {
    let at = usize::try_from(at).ok()?;
    let word = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(word.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: u64) -> Option<u64> {
    let at = usize::try_from(at).ok()?;
    let word = bytes.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(word.try_into().ok()?))
}

/// Whether `offset..offset + length` lies within `limit` bytes.
fn within(offset: u64, length: u64, limit: u64) -> bool {
    offset.checked_add(length).is_some_and(|end| end <= limit)
}

fn malformed(why: &str) -> ModelError {
    invalid(format!("a KTX2 image in the model is corrupt: {why}"))
}

/// Checks the KTX2 image in `bytes` as the module comment says, reading nothing past its end.
pub(super) fn check(bytes: &[u8]) -> Result<Container, ModelError> {
    let file = bytes.len() as u64;
    let field = |at: u64| u32_at(bytes, at).ok_or_else(|| malformed("it ends inside its header"));
    let (format, type_size) = (field(12)?, field(16)?);
    let (width, height, depth) = (field(20)?, field(24)?, field(28)?);
    let (layers, faces, levels, scheme) = (field(32)?, field(36)?, field(40)?, field(44)?);
    let (dfd, dfd_length) = (u64::from(field(48)?), u64::from(field(52)?));
    let (kvd, kvd_length) = (u64::from(field(56)?), u64::from(field(60)?));
    let sgd = u64_at(bytes, 64).ok_or_else(|| malformed("it ends inside its header"))?;
    let sgd_length = u64_at(bytes, 72).ok_or_else(|| malformed("it ends inside its header"))?;

    if format != 0 || type_size != 1 {
        return Err(unsupported(
            "a KTX2 image in the model holds texels other than Basis Universal's",
        ));
    }
    if depth != 0 || layers > 1 || faces != 1 {
        return Err(unsupported(
            "a KTX2 image in the model is a cube map, an array or a 3D image",
        ));
    }
    if width == 0 || height == 0 {
        return Err(invalid("an image in the model has no pixels"));
    }
    if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        return Err(too_large("pixels across one image", MAX_IMAGE_SIDE));
    }
    let chain = 32 - width.max(height).leading_zeros();
    if levels == 0 || levels > chain {
        return Err(malformed("its level count doesn't fit its size"));
    }
    // The two descriptor sizes Basis Universal writes: without and with an alpha slice.
    if !matches!(dfd_length, 44 | 60) || !within(dfd, dfd_length, file) {
        return Err(malformed("its data format descriptor is out of place"));
    }
    let model = bytes[dfd as usize + 12];
    let transfer = bytes[dfd as usize + 14];
    if !matches!(transfer, 1 | 2) {
        return Err(malformed(
            "its transfer function is neither linear nor sRGB",
        ));
    }
    match (model, scheme) {
        (ETC1S, BASIS_LZ) | (UASTC, NO_SUPERCOMPRESSION | ZSTANDARD) => {}
        (ETC1S | UASTC, _) => return Err(malformed("its supercompression doesn't fit its texels")),
        _ => {
            return Err(unsupported(
                "a KTX2 image in the model holds texels other than ETC1S or UASTC",
            ));
        }
    }
    // Entries of four-byte lengths, each padded to four bytes, which the transcoder walks.
    if !within(kvd, kvd_length, file) || kvd % 4 != 0 || kvd_length % 4 != 0 {
        return Err(malformed("its key and value data is out of place"));
    }

    let mut ranges = Vec::with_capacity(levels as usize);
    for level in 0..u64::from(levels) {
        let entry = 80 + level * LEVEL_ENTRY;
        let (Some(offset), Some(length), Some(uncompressed)) = (
            u64_at(bytes, entry),
            u64_at(bytes, entry + 8),
            u64_at(bytes, entry + 16),
        ) else {
            return Err(malformed("it ends inside its level index"));
        };
        if !within(offset, length, file) {
            return Err(malformed("a level reaches past its end"));
        }
        let (blocks_x, blocks_y) = (
            u64::from((width >> level).max(1).div_ceil(4)),
            u64::from((height >> level).max(1).div_ceil(4)),
        );
        let fits = match scheme {
            BASIS_LZ => uncompressed == 0,
            NO_SUPERCOMPRESSION => length == blocks_x * blocks_y * 16,
            _ => uncompressed == blocks_x * blocks_y * 16,
        };
        if !fits {
            return Err(malformed("a level's length doesn't fit its size"));
        }
        ranges.push(length);
    }

    if scheme == BASIS_LZ {
        if !within(sgd, sgd_length, file) {
            return Err(malformed("its global data is out of place"));
        }
        let global = |at: u64| u32_at(bytes, sgd + at).map(u64::from);
        let (Some(endpoints), Some(selectors), Some(tables)) = (global(4), global(8), global(12))
        else {
            return Err(malformed("its global data is too short"));
        };
        let descriptors = u64::from(levels) * IMAGE_DESCRIPTOR;
        if (20 + descriptors + endpoints + selectors + tables) > sgd_length {
            return Err(malformed("its global data is too short"));
        }
        let alpha = dfd_length == 60;
        for (level, &length) in ranges.iter().enumerate() {
            let descriptor = sgd + 20 + level as u64 * IMAGE_DESCRIPTOR;
            let slice = |at: u64| -> Option<bool> {
                let offset = u64::from(u32_at(bytes, descriptor + at)?);
                let size = u64::from(u32_at(bytes, descriptor + at + 4)?);
                Some(within(offset, size, length))
            };
            if slice(4) != Some(true) || (alpha && slice(12) != Some(true)) {
                return Err(malformed("a level's slice reaches past it"));
            }
        }
    }
    Ok(Container {
        width,
        height,
        levels,
    })
}

/// The RGBA8 rows, top first, of each mip level the KTX2 image in `bytes` holds, which [`check`]
/// found to be `container`.
pub(super) fn decode(bytes: &[u8], container: Container) -> Result<Vec<Vec<u8>>, ModelError> {
    let corrupt =
        |error: basisu::Error| invalid(format!("a KTX2 image in the model is corrupt: {error:?}"));
    guarded("a KTX2 image", || {
        let transcoder = basisu::Transcoder::new(bytes).map_err(corrupt)?;
        if transcoder.base_dimensions() != (container.width, container.height)
            || transcoder.level_count() != container.levels
        {
            return Err(malformed("it changed between reads"));
        }
        let mut levels = Vec::with_capacity(container.levels as usize);
        for level in 0..container.levels {
            let rgba = transcoder
                .transcode(
                    level,
                    basisu::TargetFormat::Rgba32,
                    basisu::DecodeFlags::NONE,
                )
                .map_err(corrupt)?;
            let (width, height) = (
                (container.width >> level).max(1),
                (container.height >> level).max(1),
            );
            if rgba.len() as u64 != u64::from(width) * u64::from(height) * 4 {
                return Err(malformed("a level decoded to the wrong size"));
            }
            levels.push(rgba);
        }
        Ok(levels)
    })
}
