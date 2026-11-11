use std::{fmt, sync::Arc};

use crate::node::NodeId;

pub use super::easing::Easing;
use super::spec::ValueKind;
pub use super::spec::{BuiltinFn, GlobalVar};

#[derive(Debug, Clone, PartialEq, lumen_macros::StructuralHash)]
pub enum ExpressionValue {
    Number(f64),
    Boolean(bool),
    String(String),
    Vec2((f64, f64)),
    Vec3([f64; 3]),
    /// Straight-alpha RGBA with components in `0..=1`.
    Color([f64; 4]),
    /// Only builtins take easings and keys; no param does.
    Easing(Easing),
    Key(Box<Key>),
}

/// A keyframe made by `key(at, value, ease?)`.
#[derive(Debug, Clone, PartialEq, lumen_macros::StructuralHash)]
pub struct Key {
    pub at: f64,
    /// A number, vec2, vec3, colour or path data.
    pub value: ExpressionValue,
    /// How the curve leaves this key, or `None` for the `keyframes` default.
    pub ease: Option<Easing>,
}

/// Parses `rrggbb` or `rrggbbaa` hex digits to RGBA, with alpha opaque when it is absent.
pub(crate) fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut color = [255; 4];
    for (index, channel) in color.iter_mut().take(hex.len() / 2).enumerate() {
        *channel = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(color)
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
            Self::Vec3(_) => "vec3",
            Self::Color(_) => "color",
            Self::Easing(_) => "easing",
            Self::Key(_) => "key",
        }
    }

    /// The kind of builtin argument this value is, where booleans count as numbers.
    pub fn kind(&self) -> ValueKind {
        match self {
            Self::Number(_) | Self::Boolean(_) => ValueKind::Number,
            Self::String(_) => ValueKind::String,
            Self::Vec2(_) => ValueKind::Vec2,
            Self::Vec3(_) => ValueKind::Vec3,
            Self::Color(_) => ValueKind::Color,
            Self::Easing(_) => ValueKind::Easing,
            Self::Key(_) => ValueKind::Key,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            Self::String(_)
            | Self::Vec2(_)
            | Self::Vec3(_)
            | Self::Color(_)
            | Self::Easing(_)
            | Self::Key(_) => None,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Self::Boolean(b) => *b,
            Self::Number(n) => is_truthy(*n),
            Self::String(s) => !s.is_empty(),
            Self::Vec2((x, y)) => is_truthy(*x) || is_truthy(*y),
            Self::Vec3(vector) => vector.iter().copied().any(is_truthy),
            Self::Color(color) => color.iter().copied().any(is_truthy),
            Self::Easing(_) | Self::Key(_) => true,
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Number(n) => n.to_string(),
            Self::Boolean(b) => b.to_string(),
            Self::Vec2((x, y)) => format!("[{x}, {y}]"),
            Self::Vec3([x, y, z]) => format!("[{x}, {y}, {z}]"),
            Self::Color(_) => {
                let [r, g, b, a] = self.as_color_u8().expect("colour value");
                format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
            }
            Self::Easing(easing) => easing.to_string(),
            Self::Key(key) => match key.ease {
                Some(ease) => format!("key({}, {}, {ease})", key.at, key.value.as_string()),
                None => format!("key({}, {})", key.at, key.value.as_string()),
            },
        }
    }

    pub fn as_vec2(&self) -> Option<(f64, f64)> {
        match self {
            Self::Vec2(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_vec3(&self) -> Option<[f64; 3]> {
        match self {
            Self::Vec3(value) => Some(*value),
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

/// Whether a number counts as true: anything but zero and NaN.
fn is_truthy(number: f64) -> bool {
    number.abs() > 0.0
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
    /// A parameter of the composition function being parsed, by position. Expanding a call
    /// replaces it, so only function bodies hold it.
    Param(usize),
}

/// What an expression reads, found once when it is built.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reads {
    pub glyph: bool,
    pub word: bool,
    pub line: bool,
    pub span: bool,
}

impl Reads {
    fn of(ast: &ExprNode) -> Self {
        let mut reads = Self::default();
        super::functions::visit(ast, &mut |node| match node {
            ExprNode::Global(GlobalVar::Glyph) => reads.glyph = true,
            ExprNode::Global(GlobalVar::Word) => reads.word = true,
            ExprNode::Global(GlobalVar::Line) => reads.line = true,
            ExprNode::Global(GlobalVar::Span) => reads.span = true,
            _ => {}
        });
        reads
    }
}

#[derive(Clone)]
pub struct Expression {
    pub(super) ast: ExprNode,
    pub source: String,
    pub reads: Reads,
    /// Levels of `ast`, which is how deep evaluating it recurses.
    pub(super) depth: usize,
    /// How `ast` is evaluated, built from it once.
    pub(super) plan: Arc<super::eval::EvalPlan>,
}

impl ExprNode {
    pub(super) fn children(&self) -> impl Iterator<Item = &ExprNode> {
        let (boxed, args): ([Option<&ExprNode>; 3], &[ExprNode]) = match self {
            Self::Unary(_, value) => ([Some(value), None, None], &[]),
            Self::Binary(lhs, _, rhs) => ([Some(lhs), Some(rhs), None], &[]),
            Self::Conditional(condition, when_true, when_false) => {
                ([Some(condition), Some(when_true), Some(when_false)], &[])
            }
            Self::Builtin(_, args) => ([None; 3], args),
            Self::Literal(_)
            | Self::Global(_)
            | Self::Node(_)
            | Self::PropertyValue(..)
            | Self::Param(_) => ([None; 3], &[]),
        };
        boxed.into_iter().flatten().chain(args)
    }

    /// This node with `rebuild` applied to each child, in order.
    pub(super) fn map_children<E>(
        &self,
        mut rebuild: impl FnMut(&ExprNode) -> Result<ExprNode, E>,
    ) -> Result<ExprNode, E> {
        Ok(match self {
            Self::Unary(op, value) => Self::Unary(*op, Box::new(rebuild(value)?)),
            Self::Binary(lhs, op, rhs) => {
                let lhs = Box::new(rebuild(lhs)?);
                Self::Binary(lhs, *op, Box::new(rebuild(rhs)?))
            }
            Self::Conditional(condition, when_true, when_false) => {
                let condition = Box::new(rebuild(condition)?);
                let when_true = Box::new(rebuild(when_true)?);
                Self::Conditional(condition, when_true, Box::new(rebuild(when_false)?))
            }
            Self::Builtin(builtin, args) => Self::Builtin(
                *builtin,
                args.iter().map(rebuild).collect::<Result<_, _>>()?,
            ),
            Self::Literal(_)
            | Self::Global(_)
            | Self::Node(_)
            | Self::PropertyValue(..)
            | Self::Param(_) => self.clone(),
        })
    }

    /// Levels of this tree, which is how deep evaluating it recurses.
    pub(super) fn depth(&self) -> usize {
        1 + self.children().map(Self::depth).max().unwrap_or(0)
    }

    /// Nodes in this tree.
    pub(super) fn count(&self) -> usize {
        1 + self.children().map(Self::count).sum::<usize>()
    }
}

impl Expression {
    /// Builds an expression from an AST, deriving its evaluation plan.
    pub(crate) fn new(ast: ExprNode, source: String) -> Self {
        Self {
            plan: Arc::new(super::eval::EvalPlan::new(&ast)),
            depth: ast.depth(),
            reads: Reads::of(&ast),
            ast,
            source,
        }
    }

    pub fn ast(&self) -> &ExprNode {
        &self.ast
    }

    /// Whether it may give a different value at a different frame, or reads something beyond
    /// the frame, composition and its own scope that could change: another node, a template
    /// prop, or text measured.
    pub fn is_frame_dependent(&self) -> bool {
        let mut dependent = false;
        super::functions::visit(&self.ast, &mut |node| {
            dependent |= match node {
                ExprNode::Global(global) => matches!(global, GlobalVar::Frame | GlobalVar::Time),
                ExprNode::Node(_) | ExprNode::PropertyValue(..) | ExprNode::Param(_) => true,
                // These count from a frame given implicitly, or read what isn't the arguments.
                ExprNode::Builtin(builtin, _) => matches!(
                    builtin,
                    BuiltinFn::Progress
                        | BuiltinFn::Tween
                        | BuiltinFn::Prop
                        | BuiltinFn::TextWidth
                        | BuiltinFn::TextHeight
                ),
                ExprNode::Literal(_)
                | ExprNode::Binary(..)
                | ExprNode::Unary(..)
                | ExprNode::Conditional(..) => false,
            };
        });
        dependent
    }
}

// The plan and reads follow from `ast`, so they are left out of comparisons and debug output.
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
    use super::{Expression, ExpressionValue, Reads};

    #[test]
    fn an_expression_knows_whether_it_depends_on_the_frame() {
        let dependent = |source: &str| Expression::parse(source).unwrap().is_frame_dependent();
        for source in [
            "instance * 2 + sin(instance / instance_count)",
            "vec3(width, if(instance > 1, 1, 2), noise(1, instance))",
            "keyframes(instance, key(0, 1), key(4, 2))",
        ] {
            assert!(!dependent(source), "{source}");
        }
        for source in [
            "frame",
            "instance + time",
            "progress(0, 10) * instance",
            "tween(0, 1, 0, 10)",
            "if(instance > 1, 1, frame)",
        ] {
            assert!(dependent(source), "{source}");
        }
    }

    #[test]
    fn an_expression_knows_what_it_reads() {
        let reads = |source: &str| Expression::parse(source).unwrap().reads;
        assert_eq!(reads("frame + 1"), Reads::default());
        assert_eq!(
            reads("glyph + if(span > 0, line, 1)"),
            Reads {
                glyph: true,
                line: true,
                span: true,
                ..Reads::default()
            }
        );
        assert!(reads("word * 2").word);
    }

    #[test]
    fn vec2_value_conversions_are_explicit() {
        let value = ExpressionValue::Vec2((12.5, -3.0));

        assert_eq!(value.type_name(), "vec2");
        assert_eq!(value.as_f64(), None);
        assert_eq!(value.as_vec2(), Some((12.5, -3.0)));
        assert_eq!(value.as_string(), "[12.5, -3]");
        assert!(value.as_bool());
        assert!(!ExpressionValue::Vec2((0.0, 0.0)).as_bool());
        assert!(ExpressionValue::Vec2((f64::EPSILON, 0.0)).as_bool());
        assert!(!ExpressionValue::Vec2((f64::NAN, -0.0)).as_bool());
    }

    #[test]
    fn vec3_value_conversions_are_explicit() {
        let value = ExpressionValue::Vec3([12.5, -3.0, 0.0]);

        assert_eq!(value.type_name(), "vec3");
        assert_eq!(value.as_vec2(), None);
        assert_eq!(value.as_vec3(), Some([12.5, -3.0, 0.0]));
        assert_eq!(value.as_string(), "[12.5, -3, 0]");
        assert!(value.as_bool());
        assert!(!ExpressionValue::Vec3([0.0, f64::NAN, -0.0]).as_bool());
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
}
