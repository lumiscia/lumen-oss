pub mod compile;
pub mod model;
pub mod source_pipeline;
pub mod time;

#[cfg(not(target_arch = "wasm32"))]
pub mod backend;
#[cfg(not(target_arch = "wasm32"))]
pub mod gpu;

pub use compile::{CompileError, CompiledTimeline, compile_project};
pub use model::*;
