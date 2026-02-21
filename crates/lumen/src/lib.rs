pub mod backend;
pub mod compile;
pub mod expression;
pub mod model;
pub mod orchestrator;

pub use backend::{FrameImage, FrameProvider, ProviderError, RenderError, Renderer};
pub use compile::{CompileError, CompiledTimeline, compile_project, compile_project_with_scale};
pub use expression::{ExprEvalError, ExprParseError, ParsedExpr, parse_expr};
pub use model::*;
pub use orchestrator::RenderOrchestrator;
