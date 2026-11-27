//! Prop values a `clip` passes to the template it instantiates.
//!
//! In JSON a value is a literal of the prop's declared kind or an `=expression`, which is
//! evaluated in the clip's own composition and frame. Literals are read against the template's
//! declarations when the composition is parsed.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    error::LumenError,
    expr::{Expression, ExpressionContext, ExpressionValue},
    json::{StringParam, classify_string},
    node::{NodeId, ParamTypeDef},
};

#[derive(Debug, Clone, Default, lumen_macros::StructuralHash)]
pub struct ClipProps(pub BTreeMap<String, ClipPropValue>);

pub(crate) static CLIP_PROPS: ParamTypeDef = ParamTypeDef {
    name: "clip_props",
    #[cfg(feature = "metadata")]
    schema: || serde_json::json!({ "$ref": "#/$defs/clipProps" }),
};

#[derive(Debug, Clone, lumen_macros::StructuralHash)]
pub enum ClipPropValue {
    /// A literal, read against the prop's declared kind.
    Literal(Value),
    Expr(Expression),
}

impl ClipProps {
    /// Parses a JSON object, naming the offending `path.prop` in errors.
    pub fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| format!("`{path}` expected an object of prop values, got {value}"))?;
        let mut props = BTreeMap::new();
        for (name, value) in object {
            let parsed = match value.as_str().and_then(classify_string) {
                Some(StringParam::Expression(source)) => Expression::parse(source)
                    .map(ClipPropValue::Expr)
                    .map_err(|error| format!("`{path}.{name}`: {error}"))?,
                Some(StringParam::EscapedLiteral(text)) => {
                    ClipPropValue::Literal(Value::from(text))
                }
                None => ClipPropValue::Literal(value.clone()),
            };
            props.insert(name.clone(), parsed);
        }
        Ok(Self(props))
    }

    pub fn to_json_value(&self) -> Value {
        let mut map = serde_json::Map::new();
        for (name, value) in &self.0 {
            map.insert(name.clone(), value.to_json_value());
        }
        Value::Object(map)
    }
}

impl ClipPropValue {
    fn to_json_value(&self) -> Value {
        match self {
            Self::Literal(Value::String(text)) if text.starts_with('=') => {
                Value::from(format!("={text}"))
            }
            Self::Literal(value) => value.clone(),
            Self::Expr(expr) => Value::from(format!("={}", expr.source)),
        }
    }
}

impl ClipProps {
    /// The JSON of these props with each expression replaced by its value at `ctx`: a string, a
    /// number, an array of numbers or a `[r, g, b, a]` colour. A failure is located at
    /// `field.name` of node `node_id`, as rendering locates it.
    pub(crate) fn evaluate_json(
        &self,
        node_id: NodeId,
        field: &str,
        ctx: &ExpressionContext<'_>,
    ) -> crate::Result<Value> {
        let mut map = serde_json::Map::new();
        for (name, value) in &self.0 {
            let ClipPropValue::Expr(expr) = value else {
                map.insert(name.clone(), value.to_json_value());
                continue;
            };
            let path = format!("{field}.{name}");
            let value = expr
                .evaluate(ctx)
                .map_err(|error| error.located(node_id, &path))?;
            let json = match value {
                ExpressionValue::Number(number) => Value::from(number),
                ExpressionValue::String(text) => Value::from(&*text),
                ExpressionValue::Vec2((x, y)) => Value::from(vec![x, y]),
                ExpressionValue::Vec3(vector) => Value::from(vector.to_vec()),
                ExpressionValue::Color(rgba) => Value::from(
                    rgba.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)
                        .to_vec(),
                ),
                other => {
                    return Err(LumenError::evaluate(format!(
                        "a prop can't be a {}",
                        other.type_name()
                    ))
                    .located(node_id, &path));
                }
            };
            map.insert(name.clone(), json);
        }
        Ok(Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_literals_expressions_and_escapes() {
        let value = json!({ "title": "Shapes", "index": 2, "at": "=frame / 2", "eq": "==x" });
        let props = ClipProps::from_json_value(&value, "props").unwrap();
        assert!(
            matches!(&props.0["title"], ClipPropValue::Literal(Value::String(text)) if text == "Shapes")
        );
        assert!(matches!(props.0["at"], ClipPropValue::Expr(_)));
        assert!(
            matches!(&props.0["eq"], ClipPropValue::Literal(Value::String(text)) if text == "=x")
        );
        assert_eq!(props.to_json_value(), value);
        let error = ClipProps::from_json_value(&json!([1]), "props").unwrap_err();
        assert!(error.contains("`props` expected an object"), "{error}");
    }
}
