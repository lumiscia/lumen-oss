//! FFmpeg-backed media resolvers.

mod audio;
mod image;
mod video;

pub use audio::FfmpegAudioResolver;
pub(crate) use video::yuv_color;
pub use video::{FfmpegResolverOptions, FfmpegVideoResolver};
