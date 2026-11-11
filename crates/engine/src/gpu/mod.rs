//! GPU-native graph renderer.
//!
//! Each frame walks the graph from the media output and records what its nodes draw, against
//! pooled textures, bump buffers and cached programs. Rasters are linear, premultiplied
//! [`WORKING_COLOR_FORMAT`] textures; `media_output` encodes them to sRGB for the output.

pub(crate) mod color;
pub(crate) mod frame_graph;
mod image_size;
#[cfg(feature = "ffmpeg")]
mod media;
pub(crate) mod params;
mod renderer;
#[cfg(test)]
mod shader_layouts;
#[cfg(any(
    all(target_os = "macos", feature = "ffmpeg", feature = "metal"),
    all(target_os = "linux", feature = "ffmpeg", feature = "vulkan")
))]
mod target;
mod types;
mod yuv;

pub use color::WORKING_COLOR_FORMAT;
pub use renderer::{
    DEFAULT_MAX_TEXT_GLYPHS, FrameGraphStats, GpuCompositionRenderer, MAX_TEXT_GLYPHS_CEILING,
    OutputAlpha,
};
#[cfg(all(target_os = "linux", feature = "ffmpeg", feature = "vulkan"))]
pub use target::VulkanVideoTarget;
#[cfg(all(
    target_os = "linux",
    feature = "ffmpeg",
    feature = "cuda",
    feature = "vulkan"
))]
pub use target::{CudaNvencTarget, CudaNvencTargetPool};
#[cfg(all(target_os = "macos", feature = "ffmpeg", feature = "metal"))]
pub use target::{MetalVideoToolboxTarget, MetalVideoToolboxTargetPool};
pub use types::{BoundFrame, MediaTextureKey, RasterHandle};
pub use yuv::{YuvConverter, YuvFormat, YuvLayout};
