pub mod compile;
pub mod model;
pub mod source_pipeline;
pub mod time;

#[cfg(any(feature = "renderer-vello", feature = "renderer-skia"))]
pub mod backend;

#[cfg(feature = "renderer-vello")]
pub mod gpu;

#[cfg(feature = "renderer-skia")]
pub mod skia;

pub use compile::{CompileError, CompiledTimeline, compile_project};
pub use model::*;
