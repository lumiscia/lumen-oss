use crate::{animation::PropertyPath, node::NodeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExpressionId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionValue {
    Number(f64),
    Boolean(bool),
    String(String),
}

impl ExpressionValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::Boolean(_) => "boolean",
            Self::String(_) => "string",
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
    TextHeight,
    TextWidth,
    Uppercase,
    Lowercase,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprNode {
    Literal(ExpressionValue),
    Binary(Box<ExprNode>, BinaryOp, Box<ExprNode>),
    Unary(UnaryOp, Box<ExprNode>),
    Builtin(BuiltinFn, Vec<ExprNode>),
    Global(GlobalVar),
    NodeProperty(NodeId, PropertyPath),
    Conditional(Box<ExprNode>, Box<ExprNode>, Box<ExprNode>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExpressionReference {
    pub node_id: NodeId,
    pub property_path: PropertyPath,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub id: ExpressionId,
    pub ast: ExprNode,
    pub references: Vec<ExpressionReference>,
    pub source: String,
}
