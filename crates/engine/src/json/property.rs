//! JSON property conversion with type coercion and expression parsing.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::{
    expr::Expression,
    node::{NodeId, PropertyDef, PropertyExpression, PropertyKind, PropertyValue, ShaderUniforms},
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
            let i =
                integral_i64(val).ok_or_else(|| anyhow!("`{name}` {}", integer_mismatch(val)))?;
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
        PropertyKind::Color => try_parse_color(val)
            .map_err(|error| anyhow!("`{name}` expected color: {error}"))
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
        PropertyKind::ShaderUniforms => ShaderUniforms::from_json_value(val, name)
            .map_err(|error| anyhow!(error))
            .map(PropertyValue::ShaderUniforms),
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

/// Reads a JSON number as an `i64`, accepting integral floats such as `3.0`.
pub(crate) fn integral_i64(value: &Value) -> Option<i64> {
    if let Some(integer) = value.as_i64() {
        return Some(integer);
    }
    // `i64::MAX as f64` rounds up to 2^63, which is not representable as an `i64`.
    let float = value.as_f64()?;
    (float.fract() == 0.0 && (i64::MIN as f64..i64::MAX as f64).contains(&float))
        .then_some(float as i64)
}

/// Says why a JSON value is not an `i64`, for values [`integral_i64`] rejected.
pub(crate) fn integer_mismatch(value: &Value) -> String {
    match value.as_f64() {
        Some(float) if float.is_finite() && float.fract() != 0.0 => {
            format!("must be a whole number, got {value}")
        }
        _ => format!("must be an integer within the i64 range, got {value}"),
    }
}

/// Params that were renamed or removed, with the hint shown when a composition
/// still uses them: `(node kind, param, hint)`.
const RETIRED_PARAMS: &[(&str, &str, &str)] = &[(
    "wgsl_shader",
    "bindings",
    "`wgsl_shader` takes a `uniforms` object instead",
)];

/// The error for a param a node does not have, with a hint for retired params.
pub fn unknown_param_error(kind: &str, key: &str, id: NodeId) -> anyhow::Error {
    let hint = RETIRED_PARAMS
        .iter()
        .find(|(retired_kind, retired_key, _)| *retired_kind == kind && *retired_key == key)
        .map(|(_, _, hint)| format!("; {hint}"))
        .unwrap_or_default();
    anyhow!("unknown param `{key}` on node {id}{hint}")
}

/// Parses `[r, g, b]`, `[r, g, b, a]` with integer components in 0–255, or a
/// `#RRGGBB` / `#RRGGBBAA` string.
pub fn parse_color(val: &Value) -> Option<[u8; 4]> {
    try_parse_color(val).ok()
}

/// Like [`parse_color`], but says why a value is not a color.
pub fn try_parse_color(val: &Value) -> Result<[u8; 4], String> {
    if let Some(arr) = val.as_array() {
        let channel = |index: usize| {
            arr[index]
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| {
                    format!(
                        "component {index} must be an integer from 0 to 255, got {}",
                        arr[index]
                    )
                })
        };
        return match arr.len() {
            3 => Ok([channel(0)?, channel(1)?, channel(2)?, 255]),
            4 => Ok([channel(0)?, channel(1)?, channel(2)?, channel(3)?]),
            length => Err(format!(
                "color array must have 3 or 4 components, got {length}"
            )),
        };
    }
    let text = val
        .as_str()
        .ok_or_else(|| format!("color must be an array or a #RRGGBB string, got {val}"))?;
    let hex = text
        .strip_prefix('#')
        .ok_or_else(|| format!("color string `{text}` must start with `#`"))?;
    if !matches!(hex.len(), 6 | 8) {
        return Err(format!("hex color `{text}` must be #RRGGBB or #RRGGBBAA"));
    }
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("hex color `{text}` contains a non-hex digit"));
    }
    let byte = |index: usize| {
        u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|error| error.to_string())
    };
    Ok([
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
            assert!(try_parse_color(&value).is_err(), "{value}");
        }
    }

    #[test]
    fn color_errors_name_the_offending_part() {
        let error = try_parse_color(&serde_json::json!([0, 256, 0])).unwrap_err();
        assert!(
            error.contains("component 1") && error.contains("256"),
            "{error}"
        );

        let error = try_parse_color(&serde_json::json!([0, 0])).unwrap_err();
        assert!(error.contains("3 or 4 components"), "{error}");

        let error = try_parse_color(&serde_json::json!("#12345")).unwrap_err();
        assert!(error.contains("#RRGGBB"), "{error}");
    }

    #[test]
    fn int_rejects_fractional_and_out_of_range_numbers() {
        assert_eq!(integral_i64(&serde_json::json!(3.0)), Some(3));
        assert_eq!(integral_i64(&serde_json::json!(-7)), Some(-7));
        for value in [
            serde_json::json!(1.5),
            serde_json::json!(1e30),
            serde_json::json!(-1e30),
            serde_json::json!(u64::MAX),
            serde_json::json!("3"),
        ] {
            assert_eq!(integral_i64(&value), None, "{value}");
        }
    }

    #[test]
    fn int_errors_distinguish_fractions_from_range() {
        assert_eq!(
            integer_mismatch(&serde_json::json!(23.84)),
            "must be a whole number, got 23.84"
        );
        assert_eq!(
            integer_mismatch(&serde_json::json!(1e30)),
            "must be an integer within the i64 range, got 1e+30"
        );
        assert_eq!(
            integer_mismatch(&serde_json::json!("3")),
            "must be an integer within the i64 range, got \"3\""
        );
    }

    #[test]
    fn retired_params_get_a_hint() {
        assert_eq!(
            unknown_param_error("wgsl_shader", "bindings", NodeId(4)).to_string(),
            "unknown param `bindings` on node 4; `wgsl_shader` takes a `uniforms` object instead"
        );
        assert_eq!(
            unknown_param_error("wgsl_shader", "nope", NodeId(4)).to_string(),
            "unknown param `nope` on node 4"
        );
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
