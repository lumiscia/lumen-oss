//! GPU-native graph compiler and renderer.
//!
//! Nodes compile into stable GPU resources and pass templates; frame-varying
//! values bind through uniform/uploads without rebuilding the render graph.

pub(crate) mod compiler;
mod renderer;
mod types;

pub use compiler::{CompileContext, FrameBindContext, GpuCompileNode, GpuFrameBindNode};
pub use renderer::GpuCompositionRenderer;
pub use types::{
    AlphaMode, BoundFrame, CompiledComposition, CompiledOutput, FrameBinding, RasterHandle,
    RasterMetadata,
};
