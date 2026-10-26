//! Expression AST, parser, and runtime evaluator for property-driven values.

pub mod ast;
pub mod builtins;
pub mod easing;
pub mod eval;
pub mod functions;
pub mod parser;
pub mod spec;

use crate::{error::ExpressionError, graph::Graph};

pub use ast::{
    BinaryOp, BuiltinFn, ExprNode, Expression, ExpressionId, ExpressionReference, ExpressionValue,
    GlobalVar, PropertyPath, UnaryOp, VirtualPropertyId,
};
pub use easing::Easing;
pub use eval::property_value_to_expression_value;
pub use functions::FunctionScope;
pub use spec::{BUILTINS, BuiltinArg, BuiltinSpec, GLOBALS, GlobalSpec, ValueKind};

impl Expression {
    pub fn parse(source: &str) -> Result<Self, ExpressionError> {
        parser::parse_expression(source)
    }
}

#[derive(Debug)]
pub struct ExpressionContext<'a> {
    /// Composition frame being evaluated; fractional values sample between frames.
    pub frame: f64,
    pub fps: f32,
    pub width: u32,
    pub height: u32,
    pub duration_frames: u32,
    pub path: Option<String>,
    pub graph: Option<&'a Graph>,
    pub media: Option<&'a dyn crate::media::MediaStore>,
    /// The glyph being drawn, while a text node evaluates its per-glyph params.
    pub glyph: Option<GlyphScope>,
}

/// A glyph and the text it is drawn in, read by the `glyph`, `word` and `line` globals and
/// their counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphScope {
    pub item: lumen_text::TextItem,
    pub counts: lumen_text::TextItem,
}

impl Default for ExpressionContext<'_> {
    fn default() -> Self {
        Self {
            frame: 0.0,
            fps: 0.0,
            width: 0,
            height: 0,
            duration_frames: 0,
            path: None,
            graph: None,
            media: None,
            glyph: None,
        }
    }
}

impl ExpressionContext<'_> {
    pub fn time_seconds(&self) -> f64 {
        if self.fps <= 0.0 {
            0.0
        } else {
            self.frame / self.fps as f64
        }
    }

    /// This context moved by `offset` frames.
    pub fn at_subframe(&self, offset: f64) -> Self {
        Self {
            frame: self.frame + offset,
            path: self.path.clone(),
            ..*self
        }
    }
}
