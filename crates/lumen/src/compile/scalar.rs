use crate::expression::{ExprParseError, ParsedExpr, parse_expr};
use crate::model::StyleValue;

#[derive(Debug, Clone)]
pub enum CompiledScalarValue {
    Literal(f32),
    Expr(ParsedExpr),
}

#[derive(Debug, Clone)]
pub struct ScalarHandle {
    index: usize,
    fallback: f32,
}

impl ScalarHandle {
    pub fn new(index: usize, fallback: f32) -> Self {
        Self { index, fallback }
    }

    pub fn index(&self) -> usize {
        self.index
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

#[cfg(test)]
mod tests {
    use super::{CompiledScalarValue, ScalarHandle, compile_optional_scalar, compile_scalar};
    use crate::model::StyleValue;

    #[test]
    fn compiles_scaled_literal_values() {
        let value = compile_scalar(&StyleValue::Value(12.0), 2.0, true).expect("compile literal");
        assert!(matches!(value, CompiledScalarValue::Literal(24.0)));
    }

    #[test]
    fn compiles_expression_values() {
        let value = compile_scalar(&StyleValue::Expr("timeline.frame".to_string()), 1.0, false)
            .expect("compile expression");
        assert!(matches!(value, CompiledScalarValue::Expr(_)));
    }

    #[test]
    fn optional_scalar_uses_default() {
        let value = compile_optional_scalar(None, 3.0, 2.0, true).expect("compile optional");
        assert!(matches!(value, CompiledScalarValue::Literal(6.0)));
    }

    #[test]
    fn scalar_handle_exposes_index_and_fallback() {
        let handle = ScalarHandle::new(42, 7.5);
        assert_eq!(handle.index(), 42);
        assert_eq!(handle.fallback(), 7.5);
    }
}
