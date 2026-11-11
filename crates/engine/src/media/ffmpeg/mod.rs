//! FFmpeg-backed media resolvers.

mod audio;
mod image;
mod video;

pub use audio::FfmpegAudioResolver;
#[cfg(any(
    all(target_os = "linux", feature = "vulkan"),
    all(target_os = "macos", feature = "metal")
))]
pub(crate) use video::yuv_color;
pub use video::{FfmpegResolverOptions, FfmpegVideoResolver};

use lumen_ffmpeg::{FfmpegError, InputContext};

use crate::{error::MediaFailure, media::AssetKind, media::sniff_kind};

/// Why opening `source` as `wanted` media failed in FFmpeg.
pub(super) fn open_failure(source: &str, wanted: AssetKind, error: &FfmpegError) -> MediaFailure {
    let failure = if error.is_resource_exhaustion() {
        MediaFailure::decode(
            source,
            format!("failed opening the {}: {error}", wanted.name()),
        )
        .transient()
    } else if error.is_missing_file() {
        MediaFailure::not_found(source, "no such file")
    } else if error.is_unsupported() {
        MediaFailure::unsupported(source, error.message.clone())
    } else if error.lacks_stream() {
        let details = format!("the file has no {} stream", wanted.name());
        MediaFailure::wrong_kind(
            source,
            &[wanted],
            other_stream_kind(source, wanted),
            details,
        )
    } else if error.is_invalid_data() {
        match head_kind(source).filter(|found| *found != wanted) {
            Some(found) => MediaFailure::wrong_kind(
                source,
                &[wanted],
                Some(found),
                format!("the file is not {}", wanted_phrase(wanted)),
            ),
            None => MediaFailure::decode(
                source,
                format!("the file is not {} FFmpeg can read", wanted_phrase(wanted)),
            ),
        }
    } else {
        MediaFailure::decode(
            source,
            format!("failed opening the {}: {error}", wanted.name()),
        )
    };
    failure.wanting(&[wanted])
}

fn wanted_phrase(wanted: AssetKind) -> String {
    wanted.to_string()
}

/// The kind of media the file has instead of `wanted`, when it holds a stream of the other kind
/// of FFmpeg media.
fn other_stream_kind(source: &str, wanted: AssetKind) -> Option<AssetKind> {
    let input = InputContext::open(source.to_string()).ok()?;
    match wanted {
        AssetKind::Video => input.best_audio_stream().ok().map(|_| AssetKind::Audio),
        _ => input.best_video_stream().ok().map(|_| AssetKind::Video),
    }
}

/// The kind of media the start of the file at `source` says it is.
fn head_kind(source: &str) -> Option<AssetKind> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let read = std::fs::File::open(source).ok()?.read(&mut head).ok()?;
    sniff_kind(&head[..read])
}
