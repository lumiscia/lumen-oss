mod audio;
mod choice;
mod codec;
mod common;
pub(crate) mod hw;
mod muxed;
mod output;
mod preset;
mod quality;
#[cfg(test)]
mod tests;
mod video;
mod video_gpu;

pub use audio::AudioEncoderConfig;
pub use choice::{EncoderCandidate, VideoEncoderChoice};
pub use codec::{BitDepth, EncoderFamily, EncoderInfo, check_bit_depth, encoder_info};
pub use muxed::MuxedEncoder;
pub use preset::{
    BUILT_IN_PRESETS, EncodeCodec, QUALITY_CONFIG_ENV, QualityConfig, QualityRequest,
};
pub use quality::{EncoderQuality, RateControl, parse_bitrate};
pub use video::{ContainerCrop, VideoEncoderConfig};
