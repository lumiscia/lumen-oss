//! What an uploaded file is, decided by opening it with the decoders renders use.

use std::{fs, io::Read, path::Path};

use lumen_3d::model::{AnimationInfo, MaterialInfo, ModelInfo};
use lumen_engine::{
    error::{MediaError, MediaFailure, MediaReason},
    media::{
        FfmpegAudioResolver, FfmpegVideoResolver, ImageFileResolver, ImageResolver,
        VideoFrameResolver,
    },
};
use lumen_ffmpeg::{InputContext, VideoCodec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Font,
    Model,
    Environment,
}

/// What the check found. Each kind fills its own fields: images their size; video its size, frame
/// rate, frames, duration, codec and audio; audio its duration, rate and channels; fonts their
/// families and faces; environment maps their size; models their animation clips and materials.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_audio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub families: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<Vec<FontFace>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<Vec<AnimationInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materials: Option<Vec<MaterialInfo>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontFace {
    pub family: String,
    pub weight: u16,
    /// `normal`, `italic` or `oblique`.
    pub style: String,
}

/// What the probe calls the uploaded file in a failure.
const FILE: &str = "the file";

/// Why a file was rejected, in the engine's vocabulary: the reason, then the `media.*` code a
/// render would fail with.
type Rejection = Box<MediaFailure>;

fn reason(rejection: &Rejection) -> String {
    format!("{} ({})", rejection.reason.details(), rejection.code())
}

fn unreadable() -> Rejection {
    Box::new(MediaFailure::decode(
        FILE,
        "not an image, video, audio file, font, model or environment map that lumen can read",
    ))
}

fn engine_rejection(error: MediaError) -> Rejection {
    match error {
        MediaError::Failed(failure) => failure,
        error => Box::new(MediaFailure::decode(FILE, error.to_string())),
    }
}

/// The kind and metadata of the file at `path`, or why it was rejected. The extension and declared
/// content type play no part.
pub fn probe(path: &Path) -> Result<(MediaKind, MediaMetadata), String> {
    probe_file(path).map_err(|rejection| reason(&rejection))
}

fn probe_file(path: &Path) -> Result<(MediaKind, MediaMetadata), Rejection> {
    let mut head = [0_u8; 16];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut head))
        .map_err(|error| MediaFailure::from_io(FILE, FILE, &error))?;
    let head = &head[..read];
    let hide_path = |mut rejection: Rejection| {
        let details = rejection
            .reason
            .details()
            .replace(&*path.to_string_lossy(), FILE);
        rejection.reason = with_details(rejection.reason, details);
        rejection
    };

    if head.starts_with(b"glTF") {
        return probe_model(path).map(|metadata| (MediaKind::Model, metadata));
    }
    if lumen_3d::environment::is_environment_magic(head) {
        return probe_environment(path).map(|metadata| (MediaKind::Environment, metadata));
    }
    if let Some(font) = font_magic(head) {
        return probe_font(path, font).map(|metadata| (MediaKind::Font, metadata));
    }
    if is_image_magic(head) {
        let image = ImageFileResolver::open(path.to_string_lossy())
            .map_err(|error| hide_path(engine_rejection(error)))?;
        let size = image.metadata();
        return Ok((
            MediaKind::Image,
            MediaMetadata {
                width: Some(size.width),
                height: Some(size.height),
                ..MediaMetadata::default()
            },
        ));
    }
    probe_av(path).map_err(hide_path)
}

enum FontMagic {
    Sfnt,
    Woff,
}

fn font_magic(head: &[u8]) -> Option<FontMagic> {
    match head.get(..4)? {
        [0, 1, 0, 0] | b"OTTO" | b"true" | b"ttcf" => Some(FontMagic::Sfnt),
        b"wOFF" | b"wOF2" => Some(FontMagic::Woff),
        _ => None,
    }
}

/// The formats renders decode images from: PNG, JPEG, GIF, WebP, BMP, TIFF and ICO.
fn is_image_magic(head: &[u8]) -> bool {
    head.starts_with(&[0x89, b'P', b'N', b'G'])
        || head.starts_with(&[0xff, 0xd8, 0xff])
        || head.starts_with(b"GIF8")
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
        || head.starts_with(b"BM")
        || head.starts_with(b"II*\0")
        || head.starts_with(b"MM\0*")
        || head.starts_with(&[0, 0, 1, 0])
}

/// `reason` with other details.
fn with_details(reason: MediaReason, details: String) -> MediaReason {
    match reason {
        MediaReason::NotFound { .. } => MediaReason::NotFound { details },
        MediaReason::WrongKind { .. } => MediaReason::WrongKind { details },
        MediaReason::Unsupported { .. } => MediaReason::Unsupported { details },
        MediaReason::TooLarge { limit, actual, .. } => MediaReason::TooLarge {
            details,
            limit,
            actual,
        },
        MediaReason::Decode { .. } => MediaReason::Decode { details },
    }
}

fn probe_font(path: &Path, magic: FontMagic) -> Result<MediaMetadata, Rejection> {
    if matches!(magic, FontMagic::Woff) {
        return Err(Box::new(MediaFailure::unsupported(
            FILE,
            "WOFF fonts are not supported: upload a TrueType or OpenType file",
        )));
    }
    let mut database = fontdb::Database::new();
    database.load_font_file(path).map_err(|error| {
        MediaFailure::decode(FILE, format!("the font could not be read: {error}"))
    })?;
    let faces: Vec<FontFace> = database
        .faces()
        .filter_map(|face| {
            let (family, _) = face.families.first()?;
            Some(FontFace {
                family: family.clone(),
                weight: face.weight.0,
                style: match face.style {
                    fontdb::Style::Normal => "normal",
                    fontdb::Style::Italic => "italic",
                    fontdb::Style::Oblique => "oblique",
                }
                .to_string(),
            })
        })
        .collect();
    if faces.is_empty() {
        return Err(Box::new(MediaFailure::decode(
            FILE,
            "the font has no faces lumen can load",
        )));
    }
    let mut families: Vec<String> = faces.iter().map(|face| face.family.clone()).collect();
    families.dedup();
    Ok(MediaMetadata {
        families: Some(families),
        faces: Some(faces),
        ..MediaMetadata::default()
    })
}

/// A binary glTF, read as renders read it.
fn probe_model(path: &Path) -> Result<MediaMetadata, Rejection> {
    let limit = lumen_3d::model::MAX_MODEL_BYTES;
    let size = fs::metadata(path)
        .map_err(|error| MediaFailure::from_io(FILE, FILE, &error))?
        .len();
    if size > limit as u64 {
        return Err(Box::new(MediaFailure::too_large(
            FILE,
            format!(
                "the model is larger than the {} MiB a model may be",
                limit >> 20
            ),
            limit as u64,
            Some(size),
        )));
    }
    let bytes = fs::read(path).map_err(|error| MediaFailure::from_io(FILE, FILE, &error))?;
    let info = ModelInfo::from_glb(&bytes).map_err(|error| {
        use lumen_3d::model::ModelError;
        let details = format!("the model could not be read: {error}");
        match error {
            ModelError::Invalid(_) => MediaFailure::decode(FILE, details),
            ModelError::Unsupported(_) => MediaFailure::unsupported(FILE, details),
            ModelError::TooLarge { limit, actual, .. } => {
                MediaFailure::too_large(FILE, details, limit, actual)
            }
        }
    })?;
    model_metadata(info)
}

/// The most the names of a model's clips and materials may take together, in bytes.
const MAX_MODEL_NAME_BYTES: usize = 64 << 10;

/// The most clips and materials a model may list together.
const MAX_MODEL_ENTRIES: usize = 4096;

/// A model's clips and materials, unless there are more of them, or their names are longer, than
/// a media record keeps.
fn model_metadata(info: ModelInfo) -> Result<MediaMetadata, Rejection> {
    let entries = info.animations.len() + info.materials.len();
    if entries > MAX_MODEL_ENTRIES {
        return Err(Box::new(MediaFailure::too_large(
            FILE,
            format!("the model has more than {MAX_MODEL_ENTRIES} animations and materials"),
            MAX_MODEL_ENTRIES as u64,
            Some(entries as u64),
        )));
    }
    let clips = info.animations.iter().map(|clip| clip.name.as_deref());
    let materials = info
        .materials
        .iter()
        .map(|material| material.name.as_deref());
    let names: usize = clips.chain(materials).flatten().map(str::len).sum();
    if names > MAX_MODEL_NAME_BYTES {
        return Err(Box::new(MediaFailure::too_large(
            FILE,
            format!(
                "the model's animation and material names take more than {} KiB",
                MAX_MODEL_NAME_BYTES >> 10
            ),
            MAX_MODEL_NAME_BYTES as u64,
            Some(names as u64),
        )));
    }
    Ok(MediaMetadata {
        animations: Some(info.animations),
        materials: Some(info.materials),
        ..MediaMetadata::default()
    })
}

/// A Radiance or OpenEXR environment map, decoded as renders decode it.
fn probe_environment(path: &Path) -> Result<MediaMetadata, Rejection> {
    use lumen_3d::environment::EnvironmentError;

    let limit = lumen_3d::environment::MAX_ENVIRONMENT_BYTES;
    let size = fs::metadata(path)
        .map_err(|error| MediaFailure::from_io(FILE, FILE, &error))?
        .len();
    if size > limit as u64 {
        return Err(Box::new(MediaFailure::too_large(
            FILE,
            format!(
                "the environment map is larger than the {} MiB an environment map may be",
                limit >> 20
            ),
            limit as u64,
            Some(size),
        )));
    }
    let bytes = fs::read(path).map_err(|error| MediaFailure::from_io(FILE, FILE, &error))?;
    let map = lumen_3d::environment::decode(&bytes).map_err(|error| {
        let details = format!("the environment map could not be read: {error}");
        match error {
            EnvironmentError::Invalid(_) => MediaFailure::decode(FILE, details),
            EnvironmentError::Unsupported(_) => MediaFailure::unsupported(FILE, details),
            EnvironmentError::TooLarge { limit, actual, .. } => {
                MediaFailure::too_large(FILE, details, limit, actual)
            }
        }
    })?;
    Ok(MediaMetadata {
        width: Some(map.width()),
        height: Some(map.height()),
        ..MediaMetadata::default()
    })
}

/// Video when a stream of more than one frame decodes; audio when only sound does, which counts
/// a still such as cover art beside it.
fn probe_av(path: &Path) -> Result<(MediaKind, MediaMetadata), Rejection> {
    let source = path.to_string_lossy().to_string();
    let input = InputContext::open(source.as_str()).map_err(|error| {
        if error.is_unsupported() {
            Box::new(MediaFailure::unsupported(
                FILE,
                "its container format is not one lumen reads",
            ))
        } else {
            unreadable()
        }
    })?;
    let info = input.media_info();
    drop(input);
    let duration_ms = info
        .duration_us
        .map(|duration| (duration.max(0) / 1000) as u64);
    let audio = info.audio_streams.first();

    if let Some(stream) = info.video_streams.first() {
        let video = FfmpegVideoResolver::open(source.as_str());
        match video {
            Ok(video) if audio.is_none() || video.metadata().frame_count > 1 => {
                let metadata = video.metadata();
                return Ok((
                    MediaKind::Video,
                    MediaMetadata {
                        width: Some(metadata.width),
                        height: Some(metadata.height),
                        duration_ms,
                        fps: Some(metadata.fps),
                        frame_count: Some(metadata.frame_count),
                        codec: codec_name(stream.codec).map(str::to_string),
                        has_audio: Some(audio.is_some()),
                        ..MediaMetadata::default()
                    },
                ));
            }
            Err(error) if audio.is_none() => return Err(engine_rejection(error)),
            _ => {}
        }
    }
    let Some(stream) = audio else {
        return Err(unreadable());
    };
    FfmpegAudioResolver::open(source.as_str()).map_err(engine_rejection)?;
    Ok((
        MediaKind::Audio,
        MediaMetadata {
            duration_ms,
            sample_rate: Some(stream.sample_rate),
            channels: Some(stream.channels),
            ..MediaMetadata::default()
        },
    ))
}

fn codec_name(codec: VideoCodec) -> Option<&'static str> {
    match codec {
        VideoCodec::H264 => Some("h264"),
        VideoCodec::Hevc => Some("hevc"),
        VideoCodec::Av1 => Some("av1"),
        VideoCodec::Vp9 => Some("vp9"),
        VideoCodec::ProRes => Some("prores"),
        VideoCodec::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_with_oversized_names_is_rejected() {
        let named = |name: String| ModelInfo {
            animations: Vec::new(),
            materials: vec![MaterialInfo {
                index: 0,
                name: Some(name),
                has_base_color_texture: false,
            }],
        };
        assert!(model_metadata(named("a".repeat(MAX_MODEL_NAME_BYTES))).is_ok());
        let error = model_metadata(named("a".repeat(MAX_MODEL_NAME_BYTES + 1))).unwrap_err();
        assert_eq!(error.code(), "media.too_large");
        assert!(error.reason.details().contains("64 KiB"), "{error}");
    }
}
