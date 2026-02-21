use crate::model::StyleValue;

#[derive(Debug, Clone)]
pub enum CompiledScalarValue {
    Literal(f32),
    Expr(String),
}

impl From<&StyleValue> for CompiledScalarValue {
    fn from(value: &StyleValue) -> Self {
        match value {
            StyleValue::Value(value) => Self::Literal(*value),
            StyleValue::Expr(expr) => Self::Expr(expr.clone()),
        }
    }
}
