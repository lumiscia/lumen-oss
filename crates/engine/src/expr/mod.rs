//! Expression AST, parser, and runtime evaluator for property-driven values.

mod ast;
mod builtins;
mod easing;
mod eval;
pub(crate) mod functions;
mod noise;
mod parser;
mod spec;

use crate::{error::ExpressionError, graph::Graph};

pub(crate) use ast::parse_hex_color;
pub use ast::{
    BinaryOp, BuiltinFn, ExprNode, Expression, ExpressionValue, GlobalVar, Reads, UnaryOp,
};
pub use functions::FunctionScope;
pub use spec::{
    BUILTINS, BuiltinArg, BuiltinSpec, Easing, FORMS, FormSpec, GLOBALS, GlobalSpec, ValueKind,
    reserved_names,
};

impl Expression {
    pub fn parse(source: &str) -> Result<Self, ExpressionError> {
        parser::parse_expression(source)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExpressionContext<'a> {
    /// Composition frame being evaluated; fractional values sample between frames.
    pub frame: f64,
    pub fps: f32,
    pub width: u32,
    pub height: u32,
    pub graph: Option<&'a Graph>,
    pub media: Option<&'a dyn crate::media::MediaStore>,
    /// The glyph being drawn, while a text node evaluates its per-glyph params.
    pub glyph: Option<GlyphScope>,
    /// The props of the template instance being evaluated, which `prop('name')` reads.
    pub props: Option<&'a crate::scope::PropValues>,
}

/// A glyph and the text it is drawn in, read by the `glyph`, `word`, `line` and `span` globals and
/// their counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphScope {
    pub item: lumen_text::TextItem,
    pub counts: lumen_text::TextItem,
}

impl ExpressionContext<'_> {
    pub fn time_seconds(&self) -> f64 {
        if self.fps <= 0.0 {
            0.0
        } else {
            self.frame / self.fps as f64
        }
    }

    /// This context outside any glyph scope, for evaluating another node's properties.
    pub fn without_glyph(&self) -> Self {
        Self {
            glyph: None,
            ..*self
        }
    }

    /// This context moved by `offset` frames.
    pub fn at_subframe(&self, offset: f64) -> Self {
        Self {
            frame: self.frame + offset,
            ..*self
        }
    }
}
