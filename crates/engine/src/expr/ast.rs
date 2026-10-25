use crate::node::NodeId;

pub use super::spec::BuiltinFn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExpressionId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PropertyPath(pub String);

impl PropertyPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VirtualPropertyId(pub u64);

impl VirtualPropertyId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Custom(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprNode {
    Literal(ExpressionValue),
    Binary(Box<ExprNode>, BinaryOp, Box<ExprNode>),
    Unary(UnaryOp, Box<ExprNode>),
    Builtin(BuiltinFn, Vec<ExprNode>),
    Global(GlobalVar),
    SymbolicPath(Vec<String>),
    Node(NodeId),
    PropertyValue(NodeId, PropertyPath),
    VirtualProperty(VirtualPropertyId),
    Conditional(Box<ExprNode>, Box<ExprNode>, Box<ExprNode>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpressionReference {
    Node {
        node_id: NodeId,
    },
    PropertyValue {
        node_id: NodeId,
        property_path: PropertyPath,
    },
    VirtualProperty {
        id: VirtualPropertyId,
    },
    SymbolicPath {
        segments: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub id: ExpressionId,
    pub ast: ExprNode,
    pub references: Vec<ExpressionReference>,
    pub source: String,
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
