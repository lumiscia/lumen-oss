//! JSON property conversion with type coercion and expression parsing.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    expr::Expression,
    node::{PropertyDef, PropertyExpression, PropertyKind, PropertyValue},
};

pub fn parse_property(
    val: &Value,
    def: Option<&PropertyDef>,
    name: &str,
) -> Result<PropertyExpression> {
    if let Some(s) = val.as_str()
        && let Some(expr_src) = s.strip_prefix('=')
    {
        let expr = Expression::parse(expr_src)
            .map_err(|e| anyhow::anyhow!("expression parse error for `{name}`: {e}"))?;
        return Ok(PropertyExpression::Expr(expr));
    }

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

pub fn parse_color(val: &Value) -> Option<[u8; 4]> {
    if let Some(arr) = val.as_array()
        && arr.len() >= 3
    {
        let r = arr[0].as_u64()? as u8;
        let g = arr[1].as_u64()? as u8;
        let b = arr[2].as_u64()? as u8;
        let a = arr.get(3).and_then(|v| v.as_u64()).unwrap_or(255) as u8;
        return Some([r, g, b, a]);
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
}
