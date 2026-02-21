use std::sync::Arc;

use thiserror::Error;

use crate::model::Project;

mod dependency;
mod operation;
mod scalar;

pub use operation::{CompiledTimeline, RuntimeFrameContext};

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("compile pipeline not implemented yet")]
    NotImplemented,
}

pub fn compile_project(_project: &Project) -> Result<Arc<CompiledTimeline>, CompileError> {
    Err(CompileError::NotImplemented)
}

pub fn compile_project_with_scale(
    _project: &Project,
    _scale: f32,
) -> Result<Arc<CompiledTimeline>, CompileError> {
    Err(CompileError::NotImplemented)
}
