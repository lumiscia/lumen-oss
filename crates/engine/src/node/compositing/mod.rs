pub mod boolean;
pub mod merge;
pub mod raster_multimerge;
pub mod switch;

use crate::{
    error::{LumenError, PropertyError},
    expr::ExpressionContext,
    node::{Deferred, DeferredValue, NodeId, PropertyValue},
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(u8)]
#[non_exhaustive]
#[delegate(kind = "enum")]
pub enum BlendMode {
    #[default]
    Normal = 0,
    Multiply = 1,
    Screen = 2,
    Overlay = 3,
    Darken = 4,
    Lighten = 5,
}

impl TryFrom<usize> for BlendMode {
    type Error = &'static str;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(BlendMode::Normal),
            1 => Ok(BlendMode::Multiply),
            2 => Ok(BlendMode::Screen),
            3 => Ok(BlendMode::Overlay),
            4 => Ok(BlendMode::Darken),
            5 => Ok(BlendMode::Lighten),
            _ => Err("failed to convert usize into BlendMode"),
        }
    }
}

impl DeferredValue for BlendMode {
    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        _ctx: &ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(_) => Err(LumenError::Property(PropertyError::InvalidType {
                node_id,
                property_path: property_path.to_string(),
                expected: "Enum",
                actual: "expression",
            })),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Int(*value as i64)
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::Int(v) => BlendMode::try_from(v as usize).ok(),
            _ => None,
        }
    }

    fn property_kind_name() -> &'static str {
        "Enum"
    }
}
