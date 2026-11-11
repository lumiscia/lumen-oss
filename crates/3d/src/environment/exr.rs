//! OpenEXR files: one part of flat scan lines or single-level tiles with R, G and B channels,
//! decoded on the calling thread.
//!
//! The headers are read and checked before any pixel is: the size against the limits, at most
//! [`MAX_CHANNELS`] channels without subsampling, blocks no larger than [`MAX_BLOCK_BYTES`]
//! decompressed, at most [`MAX_CHUNKS`] of them, and only lossless compression or PXR24. The
//! `exr` crate reads without unsafe code; a panic in it fails the map where panics unwind.

use std::cell::Cell;
use std::io::Cursor;

use exr::meta::BlockDescription;
use exr::meta::attribute::LevelMode;
use exr::prelude::{Compression, ReadChannels, ReadLayers, Vec2, read};

use super::{EnvironmentError, Equirect, Shrink, invalid, unsupported};

const MAX_CHANNELS: usize = 16;
const MAX_BLOCK_BYTES: usize = 64 << 20;
const MAX_CHUNKS: usize = 1 << 16;

fn corrupt(error: exr::error::Error) -> EnvironmentError {
    invalid(format!("the environment map is corrupt: {error}"))
}

pub(super) fn decode(bytes: &[u8]) -> Result<Equirect, EnvironmentError> {
    std::panic::catch_unwind(|| read_map(bytes))
        .unwrap_or_else(|_| Err(invalid("the environment map is corrupt")))
}

fn read_map(bytes: &[u8]) -> Result<Equirect, EnvironmentError> {
    let meta =
        exr::meta::MetaData::read_from_buffered(Cursor::new(bytes), false).map_err(corrupt)?;
    let [header] = meta.headers.as_slice() else {
        return Err(unsupported(
            "the environment map has several parts; save it as one",
        ));
    };
    if header.deep {
        return Err(unsupported("the environment map holds deep data"));
    }
    let block_pixels = match header.blocks {
        BlockDescription::ScanLines => header
            .compression
            .scan_lines_per_block()
            .checked_mul(header.layer_size.width()),
        BlockDescription::Tiles(tiles) => {
            if tiles.level_mode != LevelMode::Singular {
                return Err(unsupported(
                    "the environment map has mip or rip levels; save it without them",
                ));
            }
            tiles
                .tile_size
                .width()
                .checked_mul(tiles.tile_size.height())
        }
    };
    if !matches!(
        header.compression,
        Compression::Uncompressed
            | Compression::RLE
            | Compression::ZIP1
            | Compression::ZIP16
            | Compression::PIZ
            | Compression::PXR24
    ) {
        return Err(unsupported(format!(
            "the environment map is compressed with {}; save it with ZIP, PIZ or no compression",
            header.compression
        )));
    }
    if ["R", "G", "B"].iter().any(|&name| {
        header
            .channels
            .find_index_of_channel(&exr::prelude::Text::from(name))
            .is_none()
    }) {
        return Err(unsupported("the environment map needs R, G and B channels"));
    }
    let channels = &header.channels.list;
    if channels.len() > MAX_CHANNELS {
        return Err(unsupported(format!(
            "the environment map has more than the {MAX_CHANNELS} channels it may"
        )));
    }
    if channels
        .iter()
        .any(|channel| channel.sampling != Vec2(1, 1))
    {
        return Err(unsupported("the environment map has subsampled channels"));
    }
    let pixel_bytes: usize = channels
        .iter()
        .map(|channel| channel.sample_type.bytes_per_sample())
        .sum();
    if block_pixels
        .and_then(|pixels| pixels.checked_mul(pixel_bytes))
        .is_none_or(|bytes| bytes > MAX_BLOCK_BYTES)
        || header.chunk_count > MAX_CHUNKS
    {
        return Err(EnvironmentError::TooLarge {
            details: "the environment map's blocks are larger or more than may be read".to_string(),
            limit: MAX_BLOCK_BYTES as u64,
            actual: None,
        });
    }
    let size = header.layer_size;
    let (Ok(width), Ok(height)) = (u32::try_from(size.width()), u32::try_from(size.height()))
    else {
        return Err(EnvironmentError::TooLarge {
            details: "the environment map is too large".to_string(),
            limit: u64::from(u32::MAX),
            actual: Some(size.width().max(size.height()) as u64),
        });
    };
    let shrink = Cell::new(Some(Shrink::new(width, height)?));
    let image = read()
        .no_deep_data()
        .largest_resolution_level()
        .rgb_channels(
            |found: Vec2<usize>, _| (found == size).then(|| shrink.take()).flatten(),
            move |shrink: &mut Option<Shrink>, at: Vec2<usize>, (r, g, b): (f32, f32, f32)| {
                if let Some(shrink) = shrink
                    && at.x() < size.width()
                    && at.y() < size.height()
                {
                    shrink.add(at.x() as u32, at.y() as u32, [r, g, b]);
                }
            },
        )
        .first_valid_layer()
        .all_attributes()
        .non_parallel()
        .from_buffered(Cursor::new(bytes))
        .map_err(corrupt)?;
    image
        .layer_data
        .channel_data
        .pixels
        .map(Shrink::finish)
        .ok_or_else(|| invalid("the environment map changed size between reads"))
}
