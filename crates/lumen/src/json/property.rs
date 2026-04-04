//! JSON → [`NodeProperty`] conversion with type coercion and expression parsing.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    expr::Expression,
    node::{NodeProperty, PropertyDef, PropertyKind},
};

/// Parse a JSON value into a [`NodeProperty`], using the definition for type guidance.
pub fn parse_property(val: &Value, def: Option<&PropertyDef>, name: &str) -> Result<NodeProperty> {
    // Expression strings: values starting with `=` are parsed as expressions
    if let Some(s) = val.as_str() {
        if let Some(expr_src) = s.strip_prefix('=') {
            let expr = Expression::parse(expr_src)
                .map_err(|e| anyhow::anyhow!("expression parse error for `{name}`: {e}"))?;
            return Ok(NodeProperty::Expr(expr));
        }
    }

    if let Some(def) = def {
        return parse_typed(val, def.expected, name);
    }

    // Fallback: infer type from JSON shape
    match val {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(NodeProperty::Int(i))
            } else {
                Ok(NodeProperty::Float(n.as_f64().unwrap_or(0.0)))
            }
        }
        Value::Bool(b) => Ok(NodeProperty::Bool(*b)),
        Value::String(s) => Ok(NodeProperty::String(s.clone())),
        Value::Array(arr) => parse_color_or_vec(arr, name),
        _ => bail!("unsupported property value for `{name}`"),
    }
}

fn parse_typed(val: &Value, expected: PropertyKind, name: &str) -> Result<NodeProperty> {
    match expected {
        PropertyKind::Float => {
            let f = val
                .as_f64()
                .or_else(|| val.as_i64().map(|i| i as f64))
                .with_context(|| format!("`{name}` expected float"))?;
            Ok(NodeProperty::Float(f))
        }
        PropertyKind::Int => {
            let i = val
                .as_i64()
                .or_else(|| val.as_f64().map(|f| f as i64))
                .with_context(|| format!("`{name}` expected int"))?;
            Ok(NodeProperty::Int(i))
        }
        PropertyKind::Bool => {
            let b = val
                .as_bool()
                .or_else(|| val.as_i64().map(|i| i != 0))
                .with_context(|| format!("`{name}` expected bool"))?;
            Ok(NodeProperty::Bool(b))
        }
        PropertyKind::String => {
            let s = val
                .as_str()
                .with_context(|| format!("`{name}` expected string"))?;
            Ok(NodeProperty::String(s.to_string()))
        }
        PropertyKind::Color => parse_color(val)
            .with_context(|| format!("`{name}` expected color"))
            .map(NodeProperty::Color),
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
            Ok(NodeProperty::Vec2((x, y)))
        }
    }
}

/// Parse a color from JSON: `[r, g, b]`, `[r, g, b, a]`, `"#RRGGBB"`, or `"#RRGGBBAA"`.
pub fn parse_color(val: &Value) -> Option<[u8; 4]> {
    if let Some(arr) = val.as_array() {
        if arr.len() >= 3 {
            let r = arr[0].as_u64()? as u8;
            let g = arr[1].as_u64()? as u8;
            let b = arr[2].as_u64()? as u8;
            let a = arr.get(3).and_then(|v| v.as_u64()).unwrap_or(255) as u8;
            return Some([r, g, b, a]);
        }
    }
    if let Some(s) = val.as_str() {
        let s = s.strip_prefix('#')?;
        if s.len() == 6 || s.len() == 8 {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = if s.len() == 8 {
                u8::from_str_radix(&s[6..8], 16).ok()?
            } else {
                255
            };
            return Some([r, g, b, a]);
        }
    }
    None
}

fn parse_color_or_vec(arr: &[Value], name: &str) -> Result<NodeProperty> {
    if arr.len() == 2 {
        let x = arr[0].as_f64().with_context(|| format!("`{name}` vec2"))?;
        let y = arr[1].as_f64().with_context(|| format!("`{name}` vec2"))?;
        return Ok(NodeProperty::Vec2((x, y)));
    }
    if arr.len() >= 3 && arr.len() <= 4 {
        if let Some(c) = parse_color(&Value::Array(arr.to_vec())) {
            return Ok(NodeProperty::Color(c));
        }
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
    fn typed_property() {
        let v = serde_json::json!(3.14);
        let p = parse_property(&v, None, "test").unwrap();
        assert!(matches!(p, NodeProperty::Float(f) if (f - 3.14).abs() < 1e-10));
    }

    #[test]
    fn expression_property() {
        let v = serde_json::json!("=frame * 2");
        let p = parse_property(&v, None, "test").unwrap();
        assert!(matches!(p, NodeProperty::Expr(_)));
    }
}
