//! Expression AST, parser, and runtime evaluator for property-driven values.

pub mod ast;
pub mod builtins;
pub mod eval;
pub mod parser;

use crate::error::ExpressionError;

pub use ast::{
    BinaryOp, BuiltinFn, ExprNode, Expression, ExpressionId, ExpressionReference, ExpressionValue,
    GlobalVar, UnaryOp,
};
pub use eval::{expression_value_to_property_value, property_value_to_expression_value};

impl Expression {
    pub fn parse(source: &str) -> Result<Self, ExpressionError> {
        parser::parse_expression(source)
    }
}
