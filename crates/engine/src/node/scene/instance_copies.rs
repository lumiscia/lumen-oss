//! The literal `copies` list of an `instance_3d`.

use serde_json::{Map, Value};

use super::MAX_INSTANCES;
use crate::error::LumenError;
use crate::expr::ExpressionValue;
use crate::json::{StringParam, classify_string, try_parse_color};
use crate::node::{Delegated, ParamTypeDef, ParamValue};

/// One copy of an `instance_3d`'s `copies` list: where it sits, how it is scaled and turned, and
/// the colour that tints it.
#[derive(Debug, Clone, PartialEq, lumen_macros::StructuralHash)]
pub struct InstanceCopy {
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub scale: [f64; 3],
    /// Red, green and blue of the tint. Its alpha is ignored.
    pub tint: [u8; 3],
}

impl Default for InstanceCopy {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            tint: [255; 3],
        }
    }
}

/// The copies an `instance_3d` places, from a literal list. An empty list places none of its own.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::StructuralHash)]
pub struct InstanceCopies(pub Vec<InstanceCopy>);

pub(crate) static INSTANCE_COPIES: ParamTypeDef = ParamTypeDef {
    name: "instance_copies",
    #[cfg(feature = "metadata")]
    schema: || serde_json::json!({ "$ref": "#/$defs/instanceCopies" }),
};

/// `value`, which is a number in a copy at `path`, or why it isn't.
fn number(value: &Value, path: &str) -> Result<f64, String> {
    if let Some(number) = value.as_f64() {
        return Ok(number);
    }
    let hint = match value.as_str().and_then(classify_string) {
        Some(StringParam::Expression(_)) => {
            "; the copies are literal, so an expression can't go here"
        }
        _ => "",
    };
    Err(format!("`{path}` must be a number, got {value}{hint}"))
}

fn vec3(value: &Value, path: &str) -> Result<[f64; 3], String> {
    let items = value
        .as_array()
        .filter(|items| items.len() == 3)
        .ok_or_else(|| format!("`{path}` must be an array of 3 numbers, got {value}"))?;
    let mut vector = [0.0; 3];
    for (index, (slot, item)) in vector.iter_mut().zip(items).enumerate() {
        *slot = number(item, &format!("{path}[{index}]"))?;
    }
    Ok(vector)
}

fn tint(value: &Value, path: &str) -> Result<[u8; 3], String> {
    if value.is_object() {
        return Err(format!("`{path}` must be a solid colour, not a gradient"));
    }
    let hint = match value.as_str().and_then(classify_string) {
        Some(StringParam::Expression(_)) => {
            "; the copies are literal, so an expression can't go here"
        }
        _ => "",
    };
    let [r, g, b, _] =
        try_parse_color(value).map_err(|error| format!("`{path}`: {error}{hint}"))?;
    Ok([r, g, b])
}

impl InstanceCopy {
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let fields = value
            .as_object()
            .ok_or_else(|| format!("`{path}` must be an object, got {value}"))?;
        let mut copy = Self::default();
        for (name, value) in fields {
            let at = format!("{path}.{name}");
            match name.as_str() {
                "position" => copy.position = vec3(value, &at)?,
                "rotation" => copy.rotation = vec3(value, &at)?,
                "scale" => copy.scale = vec3(value, &at)?,
                "tint" => copy.tint = tint(value, &at)?,
                _ => {
                    return Err(format!(
                        "`{at}` isn't a copy field; a copy takes `position`, `rotation`, `scale` and `tint`"
                    ));
                }
            }
        }
        Ok(copy)
    }

    fn to_json_value(&self) -> Value {
        let [r, g, b] = self.tint;
        let mut fields = Map::new();
        fields.insert("position".into(), Value::from(self.position.to_vec()));
        fields.insert("rotation".into(), Value::from(self.rotation.to_vec()));
        fields.insert("scale".into(), Value::from(self.scale.to_vec()));
        fields.insert("tint".into(), Value::from(vec![r, g, b]));
        Value::Object(fields)
    }
}

impl ParamValue for InstanceCopies {
    const ACCEPTS_EXPRESSIONS: bool = false;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let items = value
            .as_array()
            .ok_or_else(|| format!("`{path}` must be an array of copies, got {value}"))?;
        if items.len() > MAX_INSTANCES as usize {
            return Err(format!(
                "`{path}` has {} copies, more than {MAX_INSTANCES}",
                items.len()
            ));
        }
        items
            .iter()
            .enumerate()
            .map(|(index, item)| InstanceCopy::from_json_value(item, &format!("{path}[{index}]")))
            .collect::<Result<_, _>>()
            .map(Self)
    }

    fn to_json_value(&self) -> Value {
        self.0.iter().map(InstanceCopy::to_json_value).collect()
    }

    fn from_expression(_value: ExpressionValue) -> crate::Result<Self> {
        Err(LumenError::evaluate("`copies` can't be an expression"))
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        None
    }
}

impl Delegated for InstanceCopies {
    type Delegate = crate::node::Deferred<InstanceCopies>;

    fn into_delegate(self) -> Self::Delegate {
        crate::node::Deferred::value(self)
    }
}

impl crate::node::NodeParamType for InstanceCopies {
    fn property_kind() -> crate::node::PropertyKind {
        crate::node::PropertyKind::Custom(&INSTANCE_COPIES)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn omitted_fields_take_their_defaults() {
        let copies = InstanceCopies::from_json_value(
            &json!([{ "position": [1, 2, 3], "tint": "#ff8040" }, {}]),
            "copies",
        )
        .unwrap();
        assert_eq!(
            copies.0,
            [
                InstanceCopy {
                    position: [1.0, 2.0, 3.0],
                    tint: [255, 128, 64],
                    ..InstanceCopy::default()
                },
                InstanceCopy::default()
            ]
        );
        let again = InstanceCopies::from_json_value(&copies.to_json_value(), "copies").unwrap();
        assert_eq!(again, copies);
    }
}
