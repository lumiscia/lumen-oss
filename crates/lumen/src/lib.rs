pub mod expr;
pub mod compile;
pub mod model;
pub mod source_pipeline;
pub mod time;

#[cfg(feature = "renderer-skia")]
pub mod backend;

pub use compile::{CompileError, CompiledTimeline, compile_project, compile_project_with_scale};
pub use model::*;
pub use expr::{Scalar, ExprProp, BinOp, ParsedExpr, ExprRef, ExprEvalCtx, ExprParseError, ExprEvalError, parse_expr, eval_expr};
