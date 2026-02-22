use std::collections::HashMap;
use std::ops::Range;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExpressionId(pub String);

#[derive(Debug, Clone)]
pub struct Expression {
    pub id: ExpressionId,
    pub source: String,
    pub references: Vec<ExpressionReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpressionProperty {
    X,
    Y,
    Width,
    Height,
    Opacity,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpressionReferenceTarget {
    ClipProperty {
        clip_id: String,
        property: ExpressionProperty,
    },
    LayoutNodeProperty {
        node_id: String,
        property: ExpressionProperty,
    },
}

#[derive(Debug, Clone)]
pub struct ExpressionReference {
    pub target: ExpressionReferenceTarget,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionValue {
    Number(f32),
    Boolean(bool),
    String(String),
}

#[derive(Debug, Clone, Default)]
pub struct ExpressionScope {
    pub clip_properties: HashMap<(String, ExpressionProperty), ExpressionValue>,
    pub layout_properties: HashMap<(String, ExpressionProperty), ExpressionValue>,
}

#[derive(Debug, Error)]
pub enum ExpressionError {
    #[error("expression parse is not implemented")]
    ParseNotImplemented,
    #[error("expression evaluation is not implemented")]
    EvalNotImplemented,
}

pub fn parse_expression(
    id: ExpressionId,
    source: impl Into<String>,
) -> Result<Expression, ExpressionError> {
    let source = source.into();

    Ok(Expression {
        id,
        source,
        references: Vec::new(),
    })
}

pub fn evaluate_expression(
    _expression: &Expression,
    _scope: &ExpressionScope,
) -> Result<ExpressionValue, ExpressionError> {
    Err(ExpressionError::EvalNotImplemented)
}
