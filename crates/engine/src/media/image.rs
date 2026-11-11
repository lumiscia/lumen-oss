use std::{fs, io::Cursor, path::Path, sync::Arc};

use crate::error::{MediaError, MediaFailure};

use super::{
    AssetKind, CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, Sniff,
    premultiply_rgba_in_place_if_needed, sniff,
};

/// The memory the image decoder may use for one image.
const MAX_DECODE_BYTES: u64 = 512 << 20;

#[derive(Debug, Clone)]
struct CachedImage {
    metadata: ImageMetadata,
    frame: Arc<CpuMediaFrame>,
}

#[derive(Debug, Clone)]
pub struct ImageFileResolver {
    id: String,
    cached: CachedImage,
}

impl ImageFileResolver {
    pub fn open(source: impl Into<String>) -> Result<Self, MediaError> {
        let source = source.into();
        let cached = load_cached_image(&source)?;
        Ok(Self { id: source, cached })
    }
}

impl ImageResolver for ImageFileResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> ImageMetadata {
        self.cached.metadata
    }

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.cached.frame)))
    }
}

fn load_cached_image(source: &str) -> Result<CachedImage, MediaError> {
    let encoded = fs::read(Path::new(source))
        .map_err(|err| MediaFailure::from_io(source, "image", &err).wanting(&[AssetKind::Image]))?;
    let image = image::load_from_memory(&encoded)
        .map_err(|err| image_failure(source, &encoded, &err).wanting(&[AssetKind::Image]))?;
    let straight_alpha = image.color().has_alpha() && !declares_associated_alpha(&encoded);
    let rgba = image.to_rgba8();
    let metadata = ImageMetadata {
        width: rgba.width(),
        height: rgba.height(),
    };
    let mut pixels = rgba.into_raw();
    if straight_alpha {
        premultiply_rgba_in_place_if_needed(&mut pixels);
    }
    Ok(CachedImage {
        metadata,
        frame: Arc::new(CpuMediaFrame {
            row_bytes: metadata.width as usize * 4,
            width: metadata.width,
            height: metadata.height,
            rgba: Arc::new(pixels),
        }),
    })
}

/// Why `encoded`, the file `source`, isn't an image that can be decoded.
fn image_failure(source: &str, encoded: &[u8], error: &image::ImageError) -> MediaFailure {
    use image::error::{ImageError, LimitErrorKind};

    match error {
        ImageError::Limits(limit) if matches!(limit.kind(), LimitErrorKind::InsufficientMemory) => {
            MediaFailure::too_large(
                source,
                "decoding the image needs more memory than the 512 MiB one image may",
                MAX_DECODE_BYTES,
                None,
            )
        }
        ImageError::Unsupported(_) if image::guess_format(encoded).is_err() => {
            match sniff(encoded).other_than(AssetKind::Image) {
                Some(found) => MediaFailure::wrong_kind(
                    source,
                    &[AssetKind::Image],
                    found,
                    "the file is not an image",
                ),
                None if sniff(encoded) == Sniff::Kind(AssetKind::Image) => {
                    MediaFailure::unsupported(source, "this image format is not one Lumen reads")
                }
                None => {
                    MediaFailure::decode(source, "the file is not in a recognised image format")
                }
            }
        }
        ImageError::Unsupported(_) => MediaFailure::unsupported(source, error.to_string()),
        _ => MediaFailure::decode(source, format!("failed decoding image: {error}")),
    }
}

/// Whether `encoded` is a TIFF whose `ExtraSamples` tag says its pixels are already premultiplied.
/// TIFF is the only format `image` decodes that can declare this.
fn declares_associated_alpha(encoded: &[u8]) -> bool {
    const ASSOCIATED_ALPHA: u16 = 1;
    if image::guess_format(encoded).ok() != Some(image::ImageFormat::Tiff) {
        return false;
    }
    let Ok(mut decoder) = tiff::decoder::Decoder::new(Cursor::new(encoded)) else {
        return false;
    };
    decoder
        .find_tag_unsigned_vec::<u16>(tiff::tags::Tag::ExtraSamples)
        .is_ok_and(|samples| {
            samples.is_some_and(|samples| samples.first() == Some(&ASSOCIATED_ALPHA))
        })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use ::image::{ImageBuffer, Rgba};

    #[test]
    fn image_file_resolver_caches_decoded_image_handles() {
        let path = temp_png_path();
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_pixel(2, 1, Rgba([128, 64, 32, 128]))
            .save(&path)
            .expect("write png fixture");

        let resolver = ImageFileResolver::open(path.to_string_lossy().to_string())
            .expect("open image resolver");
        let metadata = resolver.metadata();
        assert_eq!(metadata.width, 2);
        assert_eq!(metadata.height, 1);

        let MediaFrame::CpuRgba(first) = resolver.frame().expect("first resolve") else {
            panic!("expected CPU frame");
        };
        let MediaFrame::CpuRgba(second) = resolver.frame().expect("second resolve") else {
            panic!("expected CPU frame");
        };
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.width, 2);
        assert_eq!(first.height, 1);

        let _ = fs::remove_file(path);
    }

    /// One uncompressed RGBA8 pixel whose alpha `ExtraSamples` declares.
    fn tiff_pixel(pixel: [u8; 4], extra_samples: u16) -> Vec<u8> {
        const BITS_OFFSET: u32 = 134;
        const PIXEL_OFFSET: u32 = 142;
        let entries: [(u16, u16, u32, u32); 10] = [
            (256, 3, 1, 1),                    // ImageWidth
            (257, 3, 1, 1),                    // ImageLength
            (258, 3, 4, BITS_OFFSET),          // BitsPerSample
            (259, 3, 1, 1),                    // Compression: none
            (262, 3, 1, 2),                    // PhotometricInterpretation: RGB
            (273, 4, 1, PIXEL_OFFSET),         // StripOffsets
            (277, 3, 1, 4),                    // SamplesPerPixel
            (278, 3, 1, 1),                    // RowsPerStrip
            (279, 4, 1, 4),                    // StripByteCounts
            (338, 3, 1, extra_samples.into()), // ExtraSamples
        ];
        let mut bytes = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(bytes.len(), BITS_OFFSET as usize);
        for _ in 0..4 {
            bytes.extend_from_slice(&8u16.to_le_bytes());
        }
        bytes.extend_from_slice(&pixel);
        bytes
    }

    #[test]
    fn only_tiffs_declared_associated_keep_their_colour() {
        for (extra_samples, expected) in [(1, [30, 60, 100, 128]), (2, [15, 30, 50, 128])] {
            let path = temp_png_path().with_extension("tiff");
            fs::write(&path, tiff_pixel([30, 60, 100, 128], extra_samples)).expect("write tiff");
            let resolver =
                ImageFileResolver::open(path.to_string_lossy().to_string()).expect("open tiff");
            let MediaFrame::CpuRgba(frame) = resolver.frame().expect("resolve") else {
                panic!("expected CPU frame");
            };
            assert_eq!(frame.rgba[..4], expected, "ExtraSamples {extra_samples}");
            let _ = fs::remove_file(path);
        }
    }

    fn temp_png_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("lumen-image-resolver-{unique}.png"))
    }
}
