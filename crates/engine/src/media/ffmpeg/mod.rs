//! FFmpeg-backed media resolvers.

mod audio;
mod image;
mod video;

pub use audio::FfmpegAudioResolver;
pub use video::{FfmpegResolverOptions, FfmpegVideoResolver};
