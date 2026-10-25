//! JSON property conversion with type coercion and expression parsing.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::{
    expr::Expression,
    node::{PropertyDef, PropertyExpression, PropertyKind, PropertyValue},
};

/// How a JSON string parameter starting with `=` is interpreted.
pub(crate) enum StringParam<'a> {
    /// `=source`: an expression.
    Expression(&'a str),
    /// `==text`: the literal string `=text`.
    EscapedLiteral(&'a str),
}

/// Classifies strings with a leading `=`; other strings return `None`.
pub(crate) fn classify_string(text: &str) -> Option<StringParam<'_>> {
    let rest = text.strip_prefix('=')?;
    Some(if rest.starts_with('=') {
        StringParam::EscapedLiteral(rest)
    } else {
        StringParam::Expression(rest)
    })
}

/// Converts a JSON param to a property. A string starting with `=` is an
/// expression, and a leading `==` escapes a literal string starting with `=`.
pub fn parse_property(
    val: &Value,
    def: Option<&PropertyDef>,
    name: &str,
) -> Result<PropertyExpression> {
    let escaped;
    let val = match val.as_str().and_then(classify_string) {
        Some(StringParam::Expression(source)) => {
            let expr = Expression::parse(source)
                .map_err(|e| anyhow::anyhow!("expression parse error for `{name}`: {e}"))?;
            return Ok(PropertyExpression::Expr(expr));
        }
        Some(StringParam::EscapedLiteral(text)) => {
            escaped = Value::String(text.to_string());
            &escaped
        }
        None => val,
    };

    let value = if let Some(def) = def {
        parse_typed(val, def, name)?
    } else {
        parse_inferred(val, name)?
    };
    Ok(PropertyExpression::Value(value))
}

fn parse_inferred(val: &Value, name: &str) -> Result<PropertyValue> {
    match val {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(PropertyValue::Int(i))
            } else {
                Ok(PropertyValue::Float(n.as_f64().unwrap_or(0.0)))
            }
        }
        Value::Bool(b) => Ok(PropertyValue::Bool(*b)),
        Value::String(s) => Ok(PropertyValue::String(s.clone())),
        Value::Array(arr) => parse_color_or_vec(arr, name),
        _ => bail!("unsupported property value for `{name}`"),
    }
}

fn parse_typed(val: &Value, def: &PropertyDef, name: &str) -> Result<PropertyValue> {
    match def.expected {
        PropertyKind::Float => {
            let f = val
                .as_f64()
                .or_else(|| val.as_i64().map(|i| i as f64))
                .with_context(|| format!("`{name}` expected float"))?;
            Ok(PropertyValue::Float(f))
        }
        PropertyKind::Int => {
            let i = val
                .as_i64()
                .or_else(|| val.as_f64().map(|f| f as i64))
                .with_context(|| format!("`{name}` expected int"))?;
            Ok(PropertyValue::Int(i))
        }
        PropertyKind::Bool => {
            let b = val
                .as_bool()
                .or_else(|| val.as_i64().map(|i| i != 0))
                .with_context(|| format!("`{name}` expected bool"))?;
            Ok(PropertyValue::Bool(b))
        }
        PropertyKind::String => {
            let s = val
                .as_str()
                .with_context(|| format!("`{name}` expected string"))?;
            Ok(PropertyValue::String(s.to_string()))
        }
        PropertyKind::Color => parse_color(val)
            .with_context(|| format!("`{name}` expected color"))
            .map(PropertyValue::Color),
        PropertyKind::Paint => crate::node::vector::paint::Paint::from_json_value(val)
            .map_err(|error| anyhow!("`{name}`: {error}"))
            .map(PropertyValue::Paint),
        PropertyKind::Vec2 => {
            let arr = val
                .as_array()
                .with_context(|| format!("`{name}` expected [x, y]"))?;
            if arr.len() != 2 {
                bail!(
                    "`{name}` expected [x, y], got array of length {}",
                    arr.len()
                );
            }
            let x = arr[0]
                .as_f64()
                .with_context(|| format!("`{name}[0]` expected number"))?;
            let y = arr[1]
                .as_f64()
                .with_context(|| format!("`{name}[1]` expected number"))?;
            Ok(PropertyValue::Vec2((x, y)))
        }
        PropertyKind::Enum => {
            let enum_def = def
                .enum_def
                .with_context(|| format!("`{name}` missing enum definition"))?;
            let enum_name = val
                .as_str()
                .with_context(|| format!("`{name}` expected enum string"))?;
            let option = enum_def
                .options
                .iter()
                .find(|option| option.name == enum_name)
                .with_context(|| format!("`{name}` unknown enum value `{enum_name}`"))?;
            Ok(PropertyValue::Int(option.value))
        }
    }
}

/// Parses `[r, g, b]`, `[r, g, b, a]` with integer components in 0–255, or a
/// `#RRGGBB` / `#RRGGBBAA` string. Anything else returns `None`.
pub fn parse_color(val: &Value) -> Option<[u8; 4]> {
    if let Some(arr) = val.as_array() {
        let channel = |value: &Value| u8::try_from(value.as_u64()?).ok();
        return match arr.as_slice() {
            [r, g, b] => Some([channel(r)?, channel(g)?, channel(b)?, 255]),
            [r, g, b, a] => Some([channel(r)?, channel(g)?, channel(b)?, channel(a)?]),
            _ => None,
        };
    }
    let hex = val.as_str()?.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |index: usize| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok();
    Some([
        byte(0)?,
        byte(1)?,
        byte(2)?,
        if hex.len() == 8 { byte(3)? } else { 255 },
    ])
}

fn parse_color_or_vec(arr: &[Value], name: &str) -> Result<PropertyValue> {
    if arr.len() == 2 {
        let x = arr[0].as_f64().with_context(|| format!("`{name}` vec2"))?;
        let y = arr[1].as_f64().with_context(|| format!("`{name}` vec2"))?;
        return Ok(PropertyValue::Vec2((x, y)));
    }
    if arr.len() >= 3
        && arr.len() <= 4
        && let Some(c) = parse_color(&Value::Array(arr.to_vec()))
    {
        return Ok(PropertyValue::Color(c));
    }
    bail!("cannot infer type for array property `{name}`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_color() {
        let c = parse_color(&Value::String("#FF8800".into())).unwrap();
        assert_eq!(c, [255, 136, 0, 255]);

        let c = parse_color(&Value::String("#FF880080".into())).unwrap();
        assert_eq!(c, [255, 136, 0, 128]);
    }

    #[test]
    fn array_color() {
        let c = parse_color(&serde_json::json!([128, 64, 32])).unwrap();
        assert_eq!(c, [128, 64, 32, 255]);

        let c = parse_color(&serde_json::json!([128, 64, 32, 100])).unwrap();
        assert_eq!(c, [128, 64, 32, 100]);
    }

    #[test]
    fn rejects_out_of_range_and_malformed_colors() {
        for value in [
            serde_json::json!([256, 0, 0]),
            serde_json::json!([0, 0, 0, 300]),
            serde_json::json!([-1, 0, 0]),
            serde_json::json!([1.5, 0, 0]),
            serde_json::json!(["1", 0, 0]),
            serde_json::json!([0, 0, 0, "x"]),
            serde_json::json!([0, 0, 0, 255, 0]),
            serde_json::json!("#+f+f+f"),
            serde_json::json!("#ééé"),
        ] {
            assert_eq!(parse_color(&value), None, "{value}");
        }
    }

    #[test]
    fn typed_property() {
        let v = serde_json::json!(1.25);
        let p = parse_property(&v, None, "test").unwrap();
        assert!(matches!(
            p,
            PropertyExpression::Value(PropertyValue::Float(f)) if (f - 1.25).abs() < 1e-10
        ));
    }

    #[test]
    fn expression_property() {
        let v = serde_json::json!("=frame * 2");
        let p = parse_property(&v, None, "test").unwrap();
        assert!(matches!(p, PropertyExpression::Expr(_)));
    }

    #[test]
    fn leading_double_equals_is_a_literal_string() {
        for (input, expected) in [("==wow", "=wow"), ("===", "=="), ("==", "=")] {
            let p = parse_property(&serde_json::json!(input), None, "test").unwrap();
            assert!(
                matches!(&p, PropertyExpression::Value(PropertyValue::String(s)) if s == expected),
                "{input}"
            );
        }
    }
}
