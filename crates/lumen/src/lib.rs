pub mod backend;
pub mod compile;
pub mod expression;
pub mod model;
pub mod orchestrator;
pub mod time;

pub use backend::{FrameImage, FrameProvider, ProviderError, RenderError, Renderer};
pub use compile::{
    CompileError, CompiledOperation, CompiledOperationKind, CompiledTimeline, RuntimeEvalError,
    RuntimeFrameContext, compile_project, compile_project_with_scale,
};
pub use expression::{
    BinOp, ExprEvalContext, ExprEvalError, ExprParseError, ExprRef, ParsedExpr, UnaryOp, eval_expr,
    parse_expr,
};
pub use model::*;
pub use orchestrator::RenderOrchestrator;
pub use time::Rational;
