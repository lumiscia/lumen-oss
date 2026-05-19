//! GPU-native graph compiler and renderer.
//!
//! Nodes compile into stable GPU resources and pass templates; frame-varying
//! values bind through uniform/uploads without rebuilding the render graph.

mod binding;
pub(crate) mod compiler;
#[cfg(feature = "ffmpeg")]
mod media;
mod params;
mod renderer;
#[cfg(all(target_os = "macos", feature = "ffmpeg", feature = "metal"))]
mod target;
mod types;

pub use binding::FrameBindContext;
pub use compiler::{CompileContext, GpuCompileNode};
pub use renderer::GpuCompositionRenderer;
#[cfg(all(target_os = "macos", feature = "ffmpeg", feature = "metal"))]
pub use target::{MetalVideoToolboxTarget, MetalVideoToolboxTargetPool};
pub use types::{
    AlphaMode, BoundFrame, CompiledComposition, CompiledFrameBinding, CompiledOutput,
    GpuFrameBinding, MediaTextureKey, RasterHandle, RasterMetadata,
};
