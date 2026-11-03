use std::{fmt, sync::Arc};

use crate::node::NodeId;

pub use super::spec::BuiltinFn;

#[derive(Debug, Clone, PartialEq, lumen_macros::StructuralHash)]
pub enum ExpressionValue {
    Number(f64),
    Boolean(bool),
    String(String),
    Vec2((f64, f64)),
    /// Straight-alpha RGBA with components in `0..=1`.
    Color([f64; 4]),
}

impl ExpressionValue {
    pub fn color_from_u8(color: [u8; 4]) -> Self {
        Self::Color(color.map(|component| f64::from(component) / 255.0))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::Boolean(_) => "boolean",
            Self::String(_) => "string",
            Self::Vec2(_) => "vec2",
            Self::Color(_) => "color",
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            Self::String(s) => s.parse().ok(),
            Self::Vec2(_) | Self::Color(_) => None,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Self::Boolean(b) => *b,
            Self::Number(n) => n.abs() > f64::EPSILON,
            Self::String(s) => !s.is_empty(),
            Self::Vec2((x, y)) => x.abs() > f64::EPSILON || y.abs() > f64::EPSILON,
            Self::Color(color) => color.iter().any(|component| component.abs() > f64::EPSILON),
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Number(n) => n.to_string(),
            Self::Boolean(b) => b.to_string(),
            Self::Vec2((x, y)) => format!("[{x}, {y}]"),
            Self::Color(_) => {
                let [r, g, b, a] = self.as_color_u8().expect("colour value");
                format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
            }
        }
    }

    pub fn as_vec2(&self) -> Option<(f64, f64)> {
        match self {
            Self::Vec2(value) => Some(*value),
            _ => None,
        }
    }

    /// The colour as 8-bit RGBA, clamping each component to `0..=1` first.
    pub fn as_color_u8(&self) -> Option<[u8; 4]> {
        match self {
            Self::Color(color) => {
                Some(color.map(|component| (component.clamp(0.0, 1.0) * 255.0).round() as u8))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Neq,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GlobalVar {
    Frame,
    Time,
    Fps,
    Width,
    Height,
    Glyph,
    Word,
    Line,
    Span,
    GlyphCount,
    WordCount,
    LineCount,
    SpanCount,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, lumen_macros::StructuralHash)]
pub enum ExprNode {
    Literal(ExpressionValue),
    Binary(Box<ExprNode>, BinaryOp, Box<ExprNode>),
    Unary(UnaryOp, Box<ExprNode>),
    Builtin(BuiltinFn, Vec<ExprNode>),
    Global(GlobalVar),
    Node(NodeId),
    PropertyValue(NodeId, String),
    Conditional(Box<ExprNode>, Box<ExprNode>, Box<ExprNode>),
}

#[derive(Clone)]
pub struct Expression {
    pub(super) ast: ExprNode,
    pub source: String,
    /// Levels of `ast`, which is how deep evaluating it recurses.
    pub(super) depth: usize,
    /// How `ast` is evaluated, built from it once.
    pub(super) plan: Arc<super::eval::EvalPlan>,
}

impl ExprNode {
    fn depth(&self) -> usize {
        1 + match self {
            Self::Unary(_, value) => value.depth(),
            Self::Binary(lhs, _, rhs) => lhs.depth().max(rhs.depth()),
            Self::Builtin(_, args) => args.iter().map(Self::depth).max().unwrap_or(0),
            Self::Conditional(condition, when_true, when_false) => condition
                .depth()
                .max(when_true.depth())
                .max(when_false.depth()),
            Self::Literal(_) | Self::Global(_) | Self::Node(_) | Self::PropertyValue(..) => 0,
        }
    }
}

impl Expression {
    /// Builds an expression from an AST, deriving its evaluation plan.
    pub fn new(ast: ExprNode, source: String) -> Self {
        Self {
            plan: Arc::new(super::eval::EvalPlan::new(&ast)),
            depth: ast.depth(),
            ast,
            source,
        }
    }

    pub fn ast(&self) -> &ExprNode {
        &self.ast
    }
}

// The plan follows from `ast`, so it is left out of comparisons and debug output.
impl PartialEq for Expression {
    fn eq(&self, other: &Self) -> bool {
        self.ast == other.ast && self.source == other.source
    }
}

impl crate::fingerprint::StructuralHash for Expression {
    fn structural_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // The AST holds composition functions expanded at parse time, which the source names
        // only by call. Its evaluation plan is derived from it.
        self.source.structural_hash(state);
        self.ast.structural_hash(state);
    }
}

impl fmt::Debug for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Expression")
            .field("ast", &self.ast)
            .field("source", &self.source)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::ExpressionValue;

    #[test]
    fn vec2_value_conversions_are_explicit() {
        let value = ExpressionValue::Vec2((12.5, -3.0));

        assert_eq!(value.type_name(), "vec2");
        assert_eq!(value.as_f64(), None);
        assert_eq!(value.as_vec2(), Some((12.5, -3.0)));
        assert_eq!(value.as_string(), "[12.5, -3]");
        assert!(value.as_bool());
        assert!(!ExpressionValue::Vec2((0.0, 0.0)).as_bool());
        assert!(!ExpressionValue::Vec2((f64::EPSILON, 0.0)).as_bool());
    }

    #[test]
    fn color_values_convert_to_u8_and_hex() {
        let value = ExpressionValue::Color([1.0, 0.5, -0.2, 1.4]);

        assert_eq!(value.type_name(), "color");
        assert_eq!(value.as_f64(), None);
        assert_eq!(value.as_color_u8(), Some([255, 128, 0, 255]));
        assert_eq!(value.as_string(), "#ff8000ff");
        assert_eq!(
            ExpressionValue::color_from_u8([255, 0, 51, 0]),
            ExpressionValue::Color([1.0, 0.0, 0.2, 0.0])
        );
    }

    #[test]
    fn non_vector_values_do_not_coerce_to_vec2() {
        assert_eq!(ExpressionValue::Number(1.0).as_vec2(), None);
        assert_eq!(ExpressionValue::Boolean(true).as_vec2(), None);
        assert_eq!(ExpressionValue::String("1,2".to_string()).as_vec2(), None);
    }
}
