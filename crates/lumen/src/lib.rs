pub mod compile;
pub mod expr;
pub mod model;
pub mod source_pipeline;
pub mod time;

#[cfg(feature = "renderer-skia")]
pub mod backend;

pub use compile::{CompileError, CompiledTimeline, compile_project, compile_project_with_scale};
pub use expr::{
    BinOp, ExprEvalCtx, ExprEvalError, ExprParseError, ExprProp, ExprRef, ParsedExpr, Scalar,
    UnaryOp, eval_expr, parse_expr,
};
pub use model::*;
