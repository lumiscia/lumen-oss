use crate::expression::{ExprParseError, ParsedExpr, parse_expr};
use crate::model::StyleValue;

#[derive(Debug, Clone)]
pub enum CompiledScalarValue {
    Literal(f32),
    Expr(ParsedExpr),
}

impl CompiledScalarValue {
    pub fn is_expr(&self) -> bool {
        matches!(self, Self::Expr(_))
    }
}

#[derive(Debug, Clone)]
pub struct ScalarHandle {
    path: String,
    fallback: f32,
}

impl ScalarHandle {
    pub fn new(path: String, fallback: f32) -> Self {
        Self { path, fallback }
    }

    pub fn path(&self) -> &str {
        self.path.as_str()
    }

    pub fn fallback(&self) -> f32 {
        self.fallback
    }
}

pub fn compile_scalar(
    value: &StyleValue,
    scale: f32,
    spatial: bool,
) -> Result<CompiledScalarValue, ExprParseError> {
    match value {
        StyleValue::Value(number) => {
            let scaled = if spatial { number * scale } else { *number };
            Ok(CompiledScalarValue::Literal(scaled))
        }
        StyleValue::Expr(expr) => Ok(CompiledScalarValue::Expr(parse_expr(expr)?)),
    }
}

pub fn compile_optional_scalar(
    value: Option<&StyleValue>,
    default: f32,
    scale: f32,
    spatial: bool,
) -> Result<CompiledScalarValue, ExprParseError> {
    match value {
        Some(value) => compile_scalar(value, scale, spatial),
        None => {
            let scaled = if spatial { default * scale } else { default };
            Ok(CompiledScalarValue::Literal(scaled))
        }
    }
}
