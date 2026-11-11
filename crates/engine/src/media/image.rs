use std::{fs, io::Cursor, path::Path, sync::Arc};

use image::{ColorType, DynamicImage, ImageDecoder};

use crate::error::{MediaError, MediaFailure};

use super::icc::ImageColor;
use super::{
    AssetKind, CpuMediaFrame, GamutMatrix, ImageMetadata, ImageResolver, LinearMediaFrame,
    MediaFrame, Sniff, sniff,
};

/// The memory the image decoder may use for one image.
const MAX_DECODE_BYTES: u64 = 512 << 20;

#[derive(Debug, Clone)]
struct CachedImage {
    metadata: ImageMetadata,
    frame: MediaFrame,
    color_warning: Option<String>,
}

/// An image file, decoded once. Its embedded ICC profile is honoured: 8-bit sRGB stays as it is
/// decoded, 8-bit colour with the sRGB curve in other primaries is converted on the GPU, and
/// deeper samples or other curves are converted to linear half floats here.
#[derive(Debug, Clone)]
pub struct ImageFileResolver {
    cached: CachedImage,
}

impl ImageFileResolver {
    pub fn open(source: impl Into<String>) -> Result<Self, MediaError> {
        let cached = load_cached_image(&source.into())?;
        Ok(Self { cached })
    }
}

impl ImageResolver for ImageFileResolver {
    fn metadata(&self) -> ImageMetadata {
        self.cached.metadata
    }

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        Ok(self.cached.frame.clone())
    }

    fn color_warning(&self) -> Option<&str> {
        self.cached.color_warning.as_deref()
    }
}

fn load_cached_image(source: &str) -> Result<CachedImage, MediaError> {
    let encoded = fs::read(Path::new(source))
        .map_err(|err| MediaFailure::from_io(source, "image", &err).wanting(&[AssetKind::Image]))?;
    let (image, profile) = decode(&encoded)
        .map_err(|err| image_failure(source, &encoded, &err).wanting(&[AssetKind::Image]))?;
    let has_alpha = image.color().has_alpha();
    let straight_alpha = has_alpha && !declares_associated_alpha(&encoded);
    let (color, color_warning) = match profile {
        Ok(None) => (ImageColor::SRGB, None),
        Ok(Some(profile)) => match ImageColor::of_profile(&profile, !image.color().has_color()) {
            Ok(color) => (color, None),
            Err(why) => (ImageColor::SRGB, Some(why)),
        },
        Err(error) => (
            ImageColor::SRGB,
            Some(format!(
                "its ICC profile can't be read ({error}), so it is read as sRGB"
            )),
        ),
    };
    let metadata = ImageMetadata {
        width: image.width(),
        height: image.height(),
    };
    let eight_bit = matches!(
        image.color(),
        ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8
    );
    let frame = match (eight_bit, &color.curves, color.gamut) {
        (true, None, None) => {
            let frame = rgba8_frame(image);
            if straight_alpha {
                MediaFrame::RawRgba(frame)
            } else {
                MediaFrame::CpuRgba(frame)
            }
        }
        (true, None, Some(gamut)) if straight_alpha || !has_alpha => MediaFrame::WideRgba {
            gamut: GamutMatrix(gamut.map(|row| row.map(|value| value as f32))),
            frame: rgba8_frame(image),
        },
        _ => {
            let premultiplied = has_alpha && !straight_alpha;
            let rgba = color.to_linear(image.into_rgba16().into_raw(), premultiplied);
            MediaFrame::LinearRgba(Arc::new(LinearMediaFrame {
                rgba,
                width: metadata.width,
                height: metadata.height,
            }))
        }
    };
    Ok(CachedImage {
        metadata,
        frame,
        color_warning,
    })
}

fn rgba8_frame(image: DynamicImage) -> Arc<CpuMediaFrame> {
    let rgba = image.into_rgba8();
    Arc::new(CpuMediaFrame {
        row_bytes: rgba.width() as usize * 4,
        width: rgba.width(),
        height: rgba.height(),
        rgba: Arc::new(rgba.into_raw()),
    })
}

/// The image `encoded` holds, and its embedded ICC profile, or why that can't be read.
fn decode(
    encoded: &[u8],
) -> image::ImageResult<(DynamicImage, image::ImageResult<Option<Vec<u8>>>)> {
    let mut decoder = image::ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()?
        .into_decoder()?;
    let profile = decoder.icc_profile();
    let mut limits = image::Limits::default();
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    Ok((DynamicImage::from_decoder(decoder)?, profile))
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

        let MediaFrame::RawRgba(first) = resolver.frame().expect("first resolve") else {
            panic!("expected a straight-alpha frame");
        };
        let MediaFrame::RawRgba(second) = resolver.frame().expect("second resolve") else {
            panic!("expected a straight-alpha frame");
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
    fn only_tiffs_declared_associated_are_premultiplied() {
        for (extra_samples, premultiplied) in [(1, true), (2, false)] {
            let path = temp_png_path().with_extension("tiff");
            fs::write(&path, tiff_pixel([30, 60, 100, 128], extra_samples)).expect("write tiff");
            let resolver =
                ImageFileResolver::open(path.to_string_lossy().to_string()).expect("open tiff");
            let frame = resolver.frame().expect("resolve");
            let (MediaFrame::CpuRgba(cpu) | MediaFrame::RawRgba(cpu)) = &frame else {
                panic!("expected a CPU frame");
            };
            assert_eq!(cpu.rgba[..4], [30, 60, 100, 128]);
            assert_eq!(
                matches!(frame, MediaFrame::CpuRgba(_)),
                premultiplied,
                "ExtraSamples {extra_samples}"
            );
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
