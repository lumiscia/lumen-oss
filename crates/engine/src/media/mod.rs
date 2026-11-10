//! Media resolver traits, stores, and media planning helpers.

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;

#[cfg(feature = "image")]
pub mod image;

mod asset;
mod pixels;
mod prediction;
mod resolver;
mod store;
mod yuv;

#[cfg(feature = "ffmpeg")]
pub use ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver};

pub use asset::{ASSET_SCHEME, Asset, AssetKind, AssetResolver, NoAssets, asset_id, resolve_asset};
#[cfg(feature = "image")]
pub use image::ImageFileResolver;
pub use pixels::premultiply_rgba_in_place_if_needed;
pub use prediction::{FrameRequirements, VideoFrameRequirement, collect_frame_requirements};
#[cfg(feature = "ffmpeg")]
pub use resolver::GpuVideoMediaFrame;
pub use resolver::{
    CpuMediaFrame, EnvironmentResolver, FontResolver, ImageMetadata, ImageResolver, MediaFrame,
    ModelResolver, VideoFrameResolver, VideoMetadata,
};
pub use store::{MediaStore, TextSession};
pub use yuv::{
    ChromaSubsampling, Rotation, YuvChroma, YuvColor, YuvFormat, YuvFrame, YuvLayout, YuvMatrix,
    YuvPlane, YuvPlanes,
};
pub(crate) use yuv::{PlaneShape, YuvUniform};
