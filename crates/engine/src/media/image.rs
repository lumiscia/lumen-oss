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
    let failure =
        |err: image::ImageError| image_failure(source, &encoded, &err).wanting(&[AssetKind::Image]);
    let mut decoder = image::ImageReader::new(Cursor::new(&encoded[..]))
        .with_guessed_format()
        .map_err(|err| failure(err.into()))?
        .into_decoder()
        .map_err(failure)?;
    let color_type = decoder.color_type();
    let (width, height) = decoder.dimensions();
    let has_alpha = color_type.has_alpha();
    let straight_alpha = has_alpha && !declares_associated_alpha(&encoded);
    let (color, color_warning) = match decoder.icc_profile() {
        Ok(None) => (ImageColor::SRGB, None),
        Ok(Some(profile)) => match ImageColor::of_profile(&profile, !color_type.has_color()) {
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
    let ingest = Ingest::of(color_type, &color, straight_alpha);
    let peak = ingest.peak_bytes(
        color_type,
        decoder.total_bytes(),
        u64::from(width) * u64::from(height),
    );
    let mut limits = image::Limits::default();
    limits.reserve(peak).map_err(failure)?;
    decoder.set_limits(limits).map_err(failure)?;
    let image = DynamicImage::from_decoder(decoder).map_err(failure)?;
    let linear = |rgba| {
        MediaFrame::LinearRgba(Arc::new(LinearMediaFrame {
            rgba,
            width,
            height,
        }))
    };
    let premultiplied = has_alpha && !straight_alpha;
    let frame = match ingest {
        Ingest::Rgba8 { gamut: None } => {
            let frame = rgba8_frame(image);
            if straight_alpha {
                MediaFrame::RawRgba(frame)
            } else {
                MediaFrame::CpuRgba(frame)
            }
        }
        Ingest::Rgba8 { gamut: Some(gamut) } => MediaFrame::WideRgba {
            gamut: GamutMatrix(gamut.map(|row| row.map(|value| value as f32))),
            frame: rgba8_frame(image),
        },
        Ingest::Linear16 => linear(color.to_linear(image.into_rgba16().into_raw(), premultiplied)),
        Ingest::LinearFloat => {
            linear(color.float_to_linear(&image.into_rgba32f().into_raw(), premultiplied))
        }
    };
    Ok(CachedImage {
        metadata: ImageMetadata { width, height },
        frame,
        color_warning,
    })
}

/// What an image's samples become.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ingest {
    /// 8-bit RGBA as decoded, sRGB or with the sRGB curve in `gamut`'s primaries, which the GPU
    /// converts.
    Rgba8 { gamut: Option<[[f64; 3]; 3]> },
    /// Linear half floats, converted from 16-bit samples in place.
    Linear16,
    /// Linear half floats, converted from float samples.
    LinearFloat,
}

impl Ingest {
    fn of(color_type: ColorType, color: &ImageColor, straight_alpha: bool) -> Self {
        let eight_bit = matches!(
            color_type,
            ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8
        );
        // The GPU converts primaries of straight colour only.
        let gpu_gamut = color.gamut.is_none() || straight_alpha || !color_type.has_alpha();
        match color_type {
            ColorType::Rgb32F | ColorType::Rgba32F => Self::LinearFloat,
            _ if eight_bit && color.curves.is_none() && gpu_gamut => {
                Self::Rgba8 { gamut: color.gamut }
            }
            _ => Self::Linear16,
        }
    }

    /// The memory decoding an image of `color_type`, whose samples take `decoded` bytes, and
    /// converting its `pixels` take at once. An 8-bit image's expansion to RGBA isn't counted.
    fn peak_bytes(self, color_type: ColorType, decoded: u64, pixels: u64) -> u64 {
        let (rgba_type, texel_bytes, converted) = match self {
            Self::Rgba8 { .. } => return decoded,
            Self::Linear16 => (ColorType::Rgba16, 8, 0),
            Self::LinearFloat => (ColorType::Rgba32F, 16, pixels.saturating_mul(8)),
        };
        if color_type == rgba_type {
            return decoded.saturating_add(converted);
        }
        let rgba = pixels.saturating_mul(texel_bytes);
        decoded
            .saturating_add(rgba)
            .max(rgba.saturating_add(converted))
    }
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

/// Why `encoded`, the file `source`, isn't an image that can be decoded.
fn image_failure(source: &str, encoded: &[u8], error: &image::ImageError) -> MediaFailure {
    use image::error::{ImageError, LimitErrorKind};

    match error {
        ImageError::Limits(limit) if matches!(limit.kind(), LimitErrorKind::InsufficientMemory) => {
            MediaFailure::too_large(
                source,
                "decoding and converting the image needs more memory than the 512 MiB one image \
                 may",
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

    fn open(file: &[u8], extension: &str) -> Result<ImageFileResolver, MediaError> {
        let path = temp_png_path().with_extension(extension);
        fs::write(&path, file).expect("write image");
        let resolver = ImageFileResolver::open(path.to_string_lossy().to_string());
        let _ = fs::remove_file(path);
        resolver
    }

    #[test]
    fn float_tiff_samples_keep_light_above_one_and_drop_what_isnt_a_number() {
        use ::image::{ExtendedColorType, ImageEncoder, codecs::tiff::TiffEncoder};

        let samples: [f32; 8] = [2.0, 0.5, -0.25, 1.0, f32::NAN, f32::INFINITY, 0.0, 1.0];
        let mut file = Vec::new();
        TiffEncoder::new(Cursor::new(&mut file))
            .write_image(
                bytemuck::cast_slice(&samples),
                2,
                1,
                ExtendedColorType::Rgba32F,
            )
            .expect("encode float TIFF");
        let frame = open(&file, "tiff").expect("open").frame().expect("frame");
        let MediaFrame::LinearRgba(frame) = frame else {
            panic!("a float TIFF is linear half floats");
        };
        let light: Vec<f32> = frame.rgba.iter().map(|value| value.to_f32()).collect();
        let srgb_to_linear = crate::gpu::color::srgb_to_linear;
        let close = |got: f32, expected: f32| (got - expected).abs() <= expected * 1e-3;
        assert!(
            close(light[0], srgb_to_linear(2.0)) && light[0] > 4.0,
            "{light:?}"
        );
        assert!(close(light[1], srgb_to_linear(0.5)), "{light:?}");
        assert_eq!(light[2..], [0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn the_size_limit_counts_what_converting_to_half_floats_takes() {
        use ::image::{
            ExtendedColorType, ImageEncoder,
            codecs::png::{CompressionType, FilterType, PngEncoder},
        };

        // Its samples take 243 MB, within the limit, and as 16-bit RGBA another 648 MB.
        let (width, height) = (9000, 9000);
        let mut file = Vec::new();
        let mut encoder =
            PngEncoder::new_with_quality(&mut file, CompressionType::Fast, FilterType::NoFilter);
        encoder
            .set_icc_profile(super::super::icc::tests::adobe_rgb())
            .expect("PNG takes a profile");
        encoder
            .write_image(
                &vec![0; width as usize * height as usize * 3],
                width,
                height,
                ExtendedColorType::Rgb8,
            )
            .expect("encode PNG");
        let error = open(&file, "png").expect_err("too large to convert");
        assert!(
            matches!(&error, MediaError::Failed(failure)
                if matches!(failure.reason, crate::error::MediaReason::TooLarge { .. })),
            "{error}"
        );
        // Float samples are expanded to RGBA, and that is converted to half floats beside it.
        let pixels = 1000;
        assert_eq!(
            Ingest::LinearFloat.peak_bytes(ColorType::Rgb32F, pixels * 12, pixels),
            pixels * 28
        );
        assert_eq!(
            Ingest::LinearFloat.peak_bytes(ColorType::Rgba32F, pixels * 16, pixels),
            pixels * 24
        );
    }

    fn temp_png_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("lumen-image-resolver-{unique}.png"))
    }
}
