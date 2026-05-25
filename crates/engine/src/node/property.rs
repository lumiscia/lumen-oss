use crate::{
    error::{LumenError, PropertyError},
    expr::Expression,
};

use super::{ids::NodeId, vector};

#[derive(Debug, Clone)]
pub enum PropertyValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    Color([u8; 4]),
    Paint(vector::paint::Paint),
    Vec2((f64, f64)),
    FloatVec(Vec<f64>),
    IntVec(Vec<i64>),
    StringVec(Vec<String>),
}

#[derive(Debug, Clone)]
pub enum PropertyExpression {
    Value(PropertyValue),
    Expr(Expression),
}

impl PropertyValue {
    pub(crate) fn invalid_type(
        node_id: NodeId,
        property_path: &str,
        expected: &'static str,
        actual: &'static str,
    ) -> LumenError {
        LumenError::Property(PropertyError::InvalidType {
            node_id,
            property_path: property_path.to_string(),
            expected,
            actual,
        })
    }

    pub(crate) fn coerce_float(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            Self::Int(value) => Some(*value as f64),
            Self::String(value) => value.parse().ok(),
            _ => None,
        }
    }

    pub(crate) fn coerce_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Float(value) => Some(*value as i64),
            Self::Bool(value) => Some(i64::from(*value)),
            Self::String(value) => value.parse().ok(),
            _ => None,
        }
    }

    pub(crate) fn coerce_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Int(value) => Some(*value != 0),
            Self::Float(value) => Some(*value != 0.0),
            Self::String(value) => match value.to_ascii_lowercase().as_str() {
                "true" | "1" => Some(true),
                "false" | "0" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn coerce_string(&self) -> Option<String> {
        match self {
            Self::String(value) => Some(value.clone()),
            Self::Int(value) => Some(value.to_string()),
            Self::Float(value) => Some(value.to_string()),
            Self::Bool(value) => Some(value.to_string()),
            _ => None,
        }
    }

    pub fn resolve_float(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<f64> {
        self.coerce_float().ok_or_else(|| {
            Self::invalid_type(
                node_id,
                property_path,
                "Float",
                if matches!(self, Self::String(_)) {
                    "String"
                } else {
                    "unsupported"
                },
            )
        })
    }

    pub fn resolve_int(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<i64> {
        self.coerce_int().ok_or_else(|| {
            Self::invalid_type(
                node_id,
                property_path,
                "Int",
                if matches!(self, Self::String(_)) {
                    "String"
                } else {
                    "unsupported"
                },
            )
        })
    }

    pub fn resolve_bool(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<bool> {
        self.coerce_bool()
            .ok_or_else(|| Self::invalid_type(node_id, property_path, "Bool", "unsupported"))
    }

    pub fn resolve_string(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<String> {
        self.coerce_string()
            .ok_or_else(|| Self::invalid_type(node_id, property_path, "String", "unsupported"))
    }

    pub fn resolve_color(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<[u8; 4]> {
        match self {
            Self::Color(value) => Ok(*value),
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "Color",
                "unsupported",
            )),
        }
    }

    pub fn resolve_paint(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<vector::paint::Paint> {
        match self {
            Self::Paint(value) => Ok(value.clone()),
            Self::Color(value) => Ok(vector::paint::Paint::solid(*value)),
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "Paint",
                "unsupported",
            )),
        }
    }

    pub fn resolve_vec2(
        &self,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<(f64, f64)> {
        match self {
            Self::Vec2(value) => Ok(*value),
            _ => Err(Self::invalid_type(
                node_id,
                property_path,
                "Vec2",
                "unsupported",
            )),
        }
    }
}
