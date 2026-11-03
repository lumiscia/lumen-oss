use crate::{
    error::{LumenError, PropertyError},
    expr::Expression,
};

use super::{
    clip_props::ClipProps,
    ids::NodeId,
    shader_uniforms::ShaderUniforms,
    source::text_content::{
        FontFeaturesDelegate, TextContentDelegate, TextHighlightDelegate, TextLineDelegate,
        TextStrokeDelegate,
    },
    vector,
};

#[derive(Debug, Clone, lumen_macros::StructuralHash)]
pub enum PropertyValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    Color([u8; 4]),
    Paint(vector::paint::Paint),
    Vec2((f64, f64)),
    ShaderUniforms(ShaderUniforms),
    ClipProps(ClipProps),
    // The text kinds hold their delegates, so expressions in their fields are kept.
    TextContent(Box<TextContentDelegate>),
    TextLine(Box<TextLineDelegate>),
    TextHighlight(Box<TextHighlightDelegate>),
    FontFeatures(Box<FontFeaturesDelegate>),
    TextStroke(Box<TextStrokeDelegate>),
}

#[derive(Debug, Clone, lumen_macros::StructuralHash)]
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
            Self::Float(value) => value.is_finite().then_some(*value as i64),
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
}
