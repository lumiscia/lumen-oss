//! What an uploaded file is, decided by opening it with the decoders renders use.

use std::{fs, io::Read, path::Path};

use lumen_engine::media::{
    FfmpegAudioResolver, FfmpegVideoResolver, ImageFileResolver, ImageResolver, VideoFrameResolver,
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
}

/// What the check found. Each kind fills its own fields: images their size; video its size, frame
/// rate, frames, duration, codec and audio; audio its duration, rate and channels; fonts their
/// families and faces. Models fill none.
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontFace {
    pub family: String,
    pub weight: u16,
    /// `normal`, `italic` or `oblique`.
    pub style: String,
}

const UNREADABLE: &str = "not an image, video, audio file, font or model that lumen can read";

/// The kind and metadata of the file at `path`, or why it was rejected. The extension and declared
/// content type play no part.
pub fn probe(path: &Path) -> Result<(MediaKind, MediaMetadata), String> {
    let mut head = [0_u8; 16];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut head))
        .map_err(|error| format!("the file could not be read: {error}"))?;
    let head = &head[..read];
    let hide_path = |message: String| message.replace(&*path.to_string_lossy(), "the file");

    if head.starts_with(b"glTF") {
        return probe_model(path).map(|metadata| (MediaKind::Model, metadata));
    }
    if let Some(font) = font_magic(head) {
        return probe_font(path, font).map(|metadata| (MediaKind::Font, metadata));
    }
    if is_image_magic(head) {
        let image = ImageFileResolver::open(path.to_string_lossy())
            .map_err(|error| hide_path(error.to_string()))?;
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

fn probe_font(path: &Path, magic: FontMagic) -> Result<MediaMetadata, String> {
    if matches!(magic, FontMagic::Woff) {
        return Err("WOFF fonts are not supported: upload a TrueType or OpenType file".to_string());
    }
    let mut database = fontdb::Database::new();
    database
        .load_font_file(path)
        .map_err(|error| format!("the font could not be read: {error}"))?;
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
        return Err("the font has no faces lumen can load".to_string());
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
fn probe_model(path: &Path) -> Result<MediaMetadata, String> {
    let limit = lumen_3d::model::MAX_MODEL_BYTES;
    let size = fs::metadata(path)
        .map_err(|error| format!("the file could not be read: {error}"))?
        .len();
    if size > limit as u64 {
        return Err(format!(
            "the model is larger than the {} MiB a model may be",
            limit >> 20
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("the file could not be read: {error}"))?;
    lumen_3d::model::Model::from_glb(&bytes)
        .map_err(|error| format!("the model could not be read: {error}"))?;
    Ok(MediaMetadata::default())
}

/// Video when a stream of more than one frame decodes; audio when only sound does, which counts
/// a still such as cover art beside it.
fn probe_av(path: &Path) -> Result<(MediaKind, MediaMetadata), String> {
    let source = path.to_string_lossy().to_string();
    let input = InputContext::open(source.as_str()).map_err(|_| UNREADABLE.to_string())?;
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
            Err(error) if audio.is_none() => return Err(error.to_string()),
            _ => {}
        }
    }
    let Some(stream) = audio else {
        return Err(UNREADABLE.to_string());
    };
    FfmpegAudioResolver::open(source.as_str()).map_err(|error| error.to_string())?;
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
