//! Expression type placeholders for property-driven runtime evaluation.

use crate::error::ExpressionError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExpressionId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionValue {
    Number(f64),
    Boolean(bool),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprNode {
    Literal(ExpressionValue),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub id: ExpressionId,
    pub ast: ExprNode,
}

impl Expression {
    pub fn parse(source: &str) -> Result<Self, ExpressionError> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return Err(ExpressionError::Parse {
                node_id: None,
                property_path: None,
                details: "expression cannot be empty".to_string(),
            });
        }

        Ok(Self {
            id: ExpressionId(0),
            ast: ExprNode::Literal(ExpressionValue::Number(0.0)),
        })
    }
}
