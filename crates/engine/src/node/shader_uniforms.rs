//! Shader uniform values set by field name.
//!
//! In JSON a value is a number, an array of 2-4 numbers or `=expression`s, a whole-value
//! `=expression`, or a `#rrggbb` / `#rrggbbaa` colour. Which shapes fit which field is checked
//! against the shader's uniform struct when the node compiles.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::expr::Expression;

use super::Deferred;

#[derive(Debug, Clone, Default)]
pub struct ShaderUniforms(pub BTreeMap<String, ShaderUniformValue>);

#[derive(Debug, Clone)]
pub enum ShaderUniformValue {
    Number(f64),
    /// One expression for the whole field; a `vec2` result fills two components.
    Expr(Expression),
    /// 2-4 components, each a number or a numeric expression.
    Vector(Vec<Deferred<f64>>),
    Color {
        rgb: [u8; 3],
        alpha: Option<u8>,
    },
}

impl ShaderUniforms {
    /// Parses a JSON object, naming the offending `path.field` in errors.
    pub fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| format!("`{path}` expected an object of uniform values, got {value}"))?;
        object
            .iter()
            .map(|(name, value)| {
                ShaderUniformValue::from_json_value(value)
                    .map(|value| (name.clone(), value))
                    .map_err(|error| format!("`{path}.{name}`: {error}"))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    pub fn to_json_value(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(name, value)| (name.clone(), value.to_json_value()))
                .collect(),
        )
    }
}

impl ShaderUniformValue {
    pub fn from_json_value(value: &Value) -> Result<Self, String> {
        const EXPECTED: &str =
            "expected a number, an array of 2-4 numbers, an `=expression` or a `#rrggbb` colour";
        if let Some(number) = value.as_f64() {
            return Ok(Self::Number(number));
        }
        if let Some(text) = value.as_str() {
            if let Some(expression) = expression_source(text) {
                return Expression::parse(expression)
                    .map(Self::Expr)
                    .map_err(|error| error.to_string());
            }
            if let Some(hex) = text.strip_prefix('#') {
                return parse_hex_color(hex)
                    .ok_or_else(|| format!("`{text}` is not a `#rrggbb` or `#rrggbbaa` colour"));
            }
            return Err(format!("{EXPECTED}, got `{text}`"));
        }
        let Some(items) = value.as_array() else {
            return Err(format!("{EXPECTED}, got {value}"));
        };
        if !(2..=4).contains(&items.len()) {
            return Err(format!("vectors need 2-4 components, got {}", items.len()));
        }
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                if let Some(number) = item.as_f64() {
                    return Ok(Deferred::Value(number));
                }
                item.as_str()
                    .and_then(expression_source)
                    .ok_or_else(|| {
                        format!("component {index} must be a number or `=expression`, got {item}")
                    })
                    .and_then(|source| {
                        Expression::parse(source)
                            .map(Deferred::Expr)
                            .map_err(|error| error.to_string())
                    })
            })
            .collect::<Result<_, _>>()
            .map(Self::Vector)
    }

    pub fn to_json_value(&self) -> Value {
        let expression = |expression: &Expression| Value::from(format!("={}", expression.source));
        match self {
            Self::Number(number) => Value::from(*number),
            Self::Expr(expr) => expression(expr),
            Self::Vector(components) => components
                .iter()
                .map(|component| match component {
                    Deferred::Value(number) => Value::from(*number),
                    Deferred::Expr(expr) => expression(expr),
                })
                .collect(),
            Self::Color { rgb, alpha } => {
                let mut hex = String::from("#");
                for byte in rgb.iter().chain(alpha) {
                    hex.push_str(&format!("{byte:02x}"));
                }
                Value::from(hex)
            }
        }
    }
}

fn parse_hex_color(hex: &str) -> Option<ShaderUniformValue> {
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |index: usize| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok();
    Some(ShaderUniformValue::Color {
        rgb: [byte(0)?, byte(1)?, byte(2)?],
        alpha: if hex.len() == 8 { Some(byte(3)?) } else { None },
    })
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
    fn parses_each_value_form_and_round_trips() {
        let value = json!({
            "amount": 0.5,
            "center": [960.5, "=frame * 2"],
            "offset": "=vec2(1, 2)",
            "tint": "#ff8000",
            "glow": "#ff800080",
        });
        let uniforms = ShaderUniforms::from_json_value(&value, "uniforms").unwrap();
        assert!(matches!(
            uniforms.0["amount"],
            ShaderUniformValue::Number(0.5)
        ));
        assert!(matches!(&uniforms.0["center"], ShaderUniformValue::Vector(v) if v.len() == 2));
        assert!(matches!(uniforms.0["offset"], ShaderUniformValue::Expr(_)));
        assert!(matches!(
            uniforms.0["tint"],
            ShaderUniformValue::Color {
                rgb: [255, 128, 0],
                alpha: None
            }
        ));
        assert!(matches!(
            uniforms.0["glow"],
            ShaderUniformValue::Color {
                alpha: Some(128),
                ..
            }
        ));
        assert_eq!(uniforms.to_json_value(), value);
    }

    #[test]
    fn rejects_malformed_values_by_name() {
        for (value, expected) in [
            (json!([]), "`uniforms` expected an object"),
            (json!({ "a": true }), "`uniforms.a`: expected a number"),
            (json!({ "a": "red" }), "`uniforms.a`: expected a number"),
            (json!({ "a": "==x" }), "`uniforms.a`: expected a number"),
            (
                json!({ "a": [1] }),
                "`uniforms.a`: vectors need 2-4 components, got 1",
            ),
            (json!({ "a": [1, 2, 3, 4, 5] }), "got 5"),
            (
                json!({ "a": [1, "x"] }),
                "`uniforms.a`: component 1 must be a number",
            ),
            (json!({ "a": [1, [2]] }), "component 1"),
            (
                json!({ "a": "#ff00" }),
                "`uniforms.a`: `#ff00` is not a `#rrggbb`",
            ),
            (json!({ "a": "=1 +" }), "`uniforms.a`: "),
        ] {
            let error = ShaderUniforms::from_json_value(&value, "uniforms").unwrap_err();
            assert!(error.contains(expected), "{value}: {error}");
        }
    }
}
