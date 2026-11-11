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
