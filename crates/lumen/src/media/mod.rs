//! Media resolver traits, stores, and media planning helpers.

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;

#[cfg(feature = "image")]
pub mod image;

mod pixels;
mod prediction;
mod resolver;
mod store;

#[cfg(feature = "ffmpeg")]
pub use ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver};

#[cfg(feature = "image")]
pub use image::ImageFileResolver;
pub use pixels::premultiply_rgba_in_place_if_needed;
pub use prediction::{
    FrameRequirements, RenderRequirements, VideoFrameRequirement, collect_frame_requirements,
};
pub use resolver::{ImageMetadata, ImageResolver, VideoFrameResolver, VideoMetadata};
pub use store::MediaStore;
