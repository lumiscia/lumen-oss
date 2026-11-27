//! How many copies an `instance_2d` or `instance_3d` places.

use serde_json::Value;

use crate::error::LumenError;
use crate::expr::ExpressionValue;
use crate::node::{Deferred, Delegated, NodeParamType, ParamValue, PropertyKind, truncated_int};

/// Most copies an `instance_2d` or `instance_3d` may place. It is also the most objects a frame's
/// scene may draw, so a single copy of each of that many objects fills it.
pub const MAX_INSTANCES: u32 = 100_000;

/// How many copies an instance node places, from 0 to [`MAX_INSTANCES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::StructuralHash)]
pub struct InstanceCount(pub u32);

impl InstanceCount {
    fn new(count: i64) -> Option<Self> {
        u32::try_from(count)
            .ok()
            .filter(|count| *count <= MAX_INSTANCES)
            .map(Self)
    }
}

impl ParamValue for InstanceCount {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let count = i64::from_json_value(value, path)?;
        Self::new(count).ok_or_else(|| {
            format!("`{path}` must be an integer from 0 to {MAX_INSTANCES}, got {count}")
        })
    }

    fn to_json_value(&self) -> Value {
        Value::from(self.0)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        let count = truncated_int(value)?;
        Self::new(count).ok_or_else(|| {
            LumenError::evaluate(format!(
                "{count} instances is out of range; an instance node places 0 to {MAX_INSTANCES}"
            ))
        })
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(f64::from(self.0)))
    }
}

impl Delegated for InstanceCount {
    type Delegate = Deferred<InstanceCount>;

    fn into_delegate(self) -> Self::Delegate {
        Deferred::value(self)
    }
}

impl NodeParamType for InstanceCount {
    fn property_kind() -> PropertyKind {
        PropertyKind::Int
    }
}
