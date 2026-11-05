//! GPU-native graph renderer.
//!
//! Each frame walks the graph from the media output and records what its nodes draw, against
//! pooled textures, bump buffers and cached programs.

pub(crate) mod frame_graph;
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

pub use renderer::{FrameGraphStats, GpuCompositionRenderer};
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
