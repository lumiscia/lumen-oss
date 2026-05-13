//! FFmpeg-backed media resolvers.

mod audio;
mod image;
mod video;

use std::sync::OnceLock;

use ffmpeg::util::rational::Rational;
use ffmpeg_next as ffmpeg;

use crate::error::MediaError;

pub use audio::FfmpegAudioResolver;
pub use video::{FfmpegResolverOptions, FfmpegVideoResolver};

static FFMPEG_INIT: OnceLock<Result<(), String>> = OnceLock::new();

fn ensure_ffmpeg_init() -> Result<(), MediaError> {
    let init_result = FFMPEG_INIT.get_or_init(|| ffmpeg::init().map_err(|err| err.to_string()));
    init_result.clone().map_err(|details| MediaError::Decode {
        media_source: "ffmpeg".to_string(),
        details,
    })
}

fn rational_to_f64(value: Rational) -> Option<f64> {
    let denominator = value.denominator();
    if denominator == 0 {
        return None;
    }
    Some(f64::from(value.numerator()) / f64::from(denominator))
}
