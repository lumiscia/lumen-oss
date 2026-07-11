use crate::node::NodeId;

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
}

impl ExpressionValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::Boolean(_) => "boolean",
            Self::String(_) => "string",
            Self::Vec2(_) => "vec2",
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            Self::String(s) => s.parse().ok(),
            Self::Vec2(_) => None,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Self::Boolean(b) => *b,
            Self::Number(n) => n.abs() > f64::EPSILON,
            Self::String(s) => !s.is_empty(),
            Self::Vec2((x, y)) => x.abs() > f64::EPSILON || y.abs() > f64::EPSILON,
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Number(n) => n.to_string(),
            Self::Boolean(b) => b.to_string(),
            Self::Vec2((x, y)) => format!("[{x}, {y}]"),
        }
    }

    pub fn as_vec2(&self) -> Option<(f64, f64)> {
        match self {
            Self::Vec2(value) => Some(*value),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFn {
    Min,
    Max,
    Abs,
    Floor,
    Ceil,
    Round,
    Sin,
    Cos,
    Clamp,
    Lerp,
    Pow,
    Mod,
    Fract,
    Smoothstep,
    Linear,
    Step,
    TextHeight,
    TextWidth,
    Uppercase,
    Lowercase,
    Vec2,
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
