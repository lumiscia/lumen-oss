//! Expression AST, parser, and runtime evaluator for property-driven values.

pub mod ast;
pub mod builtins;
pub mod eval;
pub mod parser;

use crate::{error::ExpressionError, graph::Graph};

pub use ast::{
    BinaryOp, BuiltinFn, ExprNode, Expression, ExpressionId, ExpressionReference, ExpressionValue,
    GlobalVar, PropertyPath, UnaryOp, VirtualPropertyId,
};
pub use eval::property_value_to_expression_value;

impl Expression {
    pub fn parse(source: &str) -> Result<Self, ExpressionError> {
        parser::parse_expression(source)
    }
}

#[derive(Debug)]
pub struct ExpressionContext<'a> {
    pub frame: u32,
    pub fps: f32,
    pub width: u32,
    pub height: u32,
    pub duration_frames: u32,
    pub path: Option<String>,
    pub graph: Option<&'a Graph>,
}

impl Default for ExpressionContext<'_> {
    fn default() -> Self {
        Self {
            frame: 0,
            fps: 0.0,
            width: 0,
            height: 0,
            duration_frames: 0,
            path: None,
            graph: None,
        }
    }
}

impl ExpressionContext<'_> {
    pub fn time_seconds(&self) -> f64 {
        if self.fps <= 0.0 {
            0.0
        } else {
            self.frame as f64 / self.fps as f64
        }
    }
}
