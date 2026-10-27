//! Prop values a `clip` passes to the template it instantiates.
//!
//! In JSON a value is a literal of the prop's declared kind or an `=expression`, which is
//! evaluated in the clip's own composition and frame. Literals are read against the template's
//! declarations when the composition is parsed.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::expr::Expression;

#[derive(Debug, Clone, Default)]
pub struct ClipProps(pub BTreeMap<String, ClipPropValue>);

#[derive(Debug, Clone)]
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
        object
            .iter()
            .map(|(name, value)| {
                let parsed = match value.as_str().and_then(expression_source) {
                    Some(source) => Expression::parse(source)
                        .map(ClipPropValue::Expr)
                        .map_err(|error| format!("`{path}.{name}`: {error}"))?,
                    None => ClipPropValue::Literal(match value.as_str() {
                        Some(text) if text.starts_with("==") => Value::from(&text[1..]),
                        _ => value.clone(),
                    }),
                };
                Ok((name.clone(), parsed))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    pub fn to_json_value(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(name, value)| {
                    let value = match value {
                        ClipPropValue::Literal(Value::String(text)) if text.starts_with('=') => {
                            Value::from(format!("={text}"))
                        }
                        ClipPropValue::Literal(value) => value.clone(),
                        ClipPropValue::Expr(expr) => Value::from(format!("={}", expr.source)),
                    };
                    (name.clone(), value)
                })
                .collect(),
        )
    }
}

/// The source of a `=expression` string; `==` escapes are not expressions.
fn expression_source(text: &str) -> Option<&str> {
    text.strip_prefix('=').filter(|rest| !rest.starts_with('='))
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
