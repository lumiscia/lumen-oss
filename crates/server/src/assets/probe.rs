//! What an uploaded file is, decided by opening it with the decoders renders use.

use std::{fs, io::Read, path::Path};

use lumen_3d::model::ModelInfo;
use lumen_engine::{
    error::{MediaError, MediaFailure},
    media::{
        AssetKind, FfmpegAudioResolver, FfmpegVideoResolver, ImageFileResolver, ImageResolver,
        Sniff, VideoFrameResolver, sniff,
    },
};
use lumen_ffmpeg::{EncodeCodec, InputContext};
use lumen_server_api::{FontFace, MediaKind, MediaMetadata};
use serde::Serialize;

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
        rejection.reason = rejection.reason.clone().with_details(details);
        rejection
    };

    match sniff(head) {
        Sniff::Kind(AssetKind::Model) => {
            return probe_model(path).map(|metadata| (MediaKind::Model, metadata));
        }
        Sniff::Kind(AssetKind::Environment) => {
            return probe_environment(path).map(|metadata| (MediaKind::Environment, metadata));
        }
        Sniff::Kind(AssetKind::Font) => {
            let woff = head.starts_with(b"wOFF") || head.starts_with(b"wOF2");
            return probe_font(path, woff).map(|metadata| (MediaKind::Font, metadata));
        }
        // HEIC and AVIF are images the renders' decoders can't open, but FFmpeg can.
        Sniff::Kind(AssetKind::Image) if head.get(4..8) != Some(b"ftyp") => {
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
        _ => {}
    }
    if is_gltf_json_file(path) {
        return Err(Box::new(MediaFailure::unsupported(
            FILE,
            "glTF JSON (.gltf) files are not read; upload a binary glTF (.glb)",
        )));
    }
    probe_av(path).map_err(hide_path)
}

/// Whether the start of the file at `path` is a glTF JSON document.
fn is_gltf_json_file(path: &Path) -> bool {
    let mut prefix = Vec::new();
    fs::File::open(path)
        .and_then(|file| {
            file.take(lumen_3d::model::GLTF_JSON_SNIFF_BYTES as u64)
                .read_to_end(&mut prefix)
        })
        .is_ok()
        && lumen_3d::model::is_gltf_json(&prefix)
}

fn probe_font(path: &Path, woff: bool) -> Result<MediaMetadata, Rejection> {
    if woff {
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

/// The most the names of a model's clips, materials and cameras may take together, in bytes.
const MAX_MODEL_NAME_BYTES: usize = 64 << 10;

/// The most clips, materials and cameras a model may list together.
const MAX_MODEL_ENTRIES: usize = 4096;

/// A model's clips, materials and cameras, unless there are more of them, or their names are
/// longer, than a media record keeps.
fn model_metadata(info: ModelInfo) -> Result<MediaMetadata, Rejection> {
    let entries = info.animations.len() + info.materials.len() + info.cameras.len();
    if entries > MAX_MODEL_ENTRIES {
        return Err(Box::new(MediaFailure::too_large(
            FILE,
            format!(
                "the model has more than {MAX_MODEL_ENTRIES} animations, materials and cameras"
            ),
            MAX_MODEL_ENTRIES as u64,
            Some(entries as u64),
        )));
    }
    let clips = info.animations.iter().map(|clip| clip.name.as_deref());
    let materials = info
        .materials
        .iter()
        .map(|material| material.name.as_deref());
    let cameras = info.cameras.iter().map(|camera| camera.name.as_deref());
    let names: usize = clips
        .chain(materials)
        .chain(cameras)
        .flatten()
        .map(str::len)
        .sum();
    if names > MAX_MODEL_NAME_BYTES {
        return Err(Box::new(MediaFailure::too_large(
            FILE,
            format!(
                "the model's animation, material and camera names take more than {} KiB",
                MAX_MODEL_NAME_BYTES >> 10
            ),
            MAX_MODEL_NAME_BYTES as u64,
            Some(names as u64),
        )));
    }
    Ok(MediaMetadata {
        animations: Some(json_list(&info.animations)),
        materials: Some(json_list(&info.materials)),
        cameras: Some(json_list(&info.cameras)),
        ..MediaMetadata::default()
    })
}

/// `items` as the JSON the API carries them in.
fn json_list<T: Serialize>(items: &[T]) -> Vec<serde_json::Value> {
    items
        .iter()
        .map(|item| serde_json::to_value(item).expect("model info serialises"))
        .collect()
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
                        codec: EncodeCodec::of(stream.codec).map(|codec| codec.name().to_string()),
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

#[cfg(test)]
mod tests {
    use lumen_3d::model::MaterialInfo;

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
            cameras: Vec::new(),
        };
        assert!(model_metadata(named("a".repeat(MAX_MODEL_NAME_BYTES))).is_ok());
        let error = model_metadata(named("a".repeat(MAX_MODEL_NAME_BYTES + 1))).unwrap_err();
        assert_eq!(error.code(), "media.too_large");
        assert!(error.reason.details().contains("64 KiB"), "{error}");
    }

    #[test]
    fn a_gltf_json_file_is_unsupported_and_other_json_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            path
        };
        let gltf = write("scene.gltf", br#"{"asset":{"version":"2.0"},"scenes":[]}"#);
        let rejection = probe_file(&gltf).unwrap_err();
        assert_eq!(rejection.code(), "media.unsupported");
        assert!(rejection.reason.details().contains(".glb"), "{rejection}");
        let mut indented = vec![b' '; 32];
        indented.extend_from_slice(br#"{"asset":{"version":"2.0"}}"#);
        let indented = write("indented.gltf", &indented);
        assert_eq!(
            probe_file(&indented).unwrap_err().code(),
            "media.unsupported"
        );
        for bytes in [&b"{garbage"[..], br#"{"name": "a", "version": "1.0"}"#] {
            let other = write("other.json", bytes);
            assert_eq!(probe_file(&other).unwrap_err().code(), "media.decode");
        }
    }
}
