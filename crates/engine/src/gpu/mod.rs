//! GPU-native graph compiler and renderer.
//!
//! Nodes compile into stable GPU resources and pass templates; frame-varying
//! values bind through uniform/uploads without rebuilding the render graph.

mod binding;
pub(crate) mod compiler;
mod params;
mod renderer;
mod types;

pub use binding::FrameBindContext;
pub use compiler::{CompileContext, GpuCompileNode};
pub use renderer::GpuCompositionRenderer;
pub use types::{
    AlphaMode, BoundFrame, CompiledComposition, CompiledFrameBinding, CompiledOutput,
    GpuFrameBinding, MediaTextureKey, RasterHandle, RasterMetadata,
};
