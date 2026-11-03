//! JSON property conversion with type coercion and expression parsing.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::{
    error::{LumenError, PropertyError},
    expr::Expression,
    node::{
        ClipProps, Deferred, DelegateValue, EnumDef, NodeId, OptionalDelegate, Override,
        PropertyDef, PropertyExpression, PropertyKind, PropertyValue, ShaderUniforms,
        source::text_content::{FontFeaturesDelegate, TextContentDelegate},
    },
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
pub fn parse_property(val: &Value, def: &PropertyDef, name: &str) -> Result<PropertyExpression> {
    let escaped;
    let val = match val.as_str().and_then(classify_string) {
        Some(StringParam::Expression(source)) => {
            let expr = Expression::parse(source)
                .map_err(|e| anyhow::anyhow!("`{name}`: `={source}`: {e}"))?;
            return Ok(PropertyExpression::Expr(expr));
        }
        Some(StringParam::EscapedLiteral(text)) => {
            escaped = Value::String(text.to_string());
            &escaped
        }
        None => val,
    };

    Ok(PropertyExpression::Value(parse_typed(val, def, name)?))
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
        PropertyKind::ClipProps => ClipProps::from_json_value(val, name)
            .map_err(|error| anyhow!(error))
            .map(PropertyValue::ClipProps),
        // `parse_property` has already classified a string, so it is the literal text.
        PropertyKind::TextContent => match val {
            Value::String(text) => Ok(TextContentDelegate::Text(Deferred::Value(text.clone()))),
            _ => TextContentDelegate::from_json_value(val)
                .map_err(|error| anyhow!("`{name}`: {error}")),
        }
        .map(|content| PropertyValue::TextContent(Box::new(content))),
        PropertyKind::TextLine => nested_from_json(val)
            .map_err(|error| anyhow!("`{name}`: {error}"))
            .map(|line| PropertyValue::TextLine(Box::new(line))),
        PropertyKind::TextHighlight => nested_from_json(val)
            .map_err(|error| anyhow!("`{name}`: {error}"))
            .map(|highlight| PropertyValue::TextHighlight(Box::new(highlight))),
        PropertyKind::FontFeatures => FontFeaturesDelegate::from_json_value(val)
            .map_err(|error| anyhow!("`{name}`: {error}"))
            .map(|features| PropertyValue::FontFeatures(Box::new(features))),
        PropertyKind::TextStroke => nested_from_json(val)
            .map_err(|error| anyhow!("`{name}`: {error}"))
            .map(|stroke| PropertyValue::TextStroke(Box::new(stroke))),
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

/// Deserializes `val`, naming the path to an error inside it.
pub(crate) fn nested_from_json<T: serde::de::DeserializeOwned>(
    val: &Value,
) -> std::result::Result<T, String> {
    serde_path_to_error::deserialize(val).map_err(|error| error.to_string())
}

/// Parses a field of a param nested in another, such as a text span's `font_size`, the way
/// node params parse.
pub(crate) fn parse_nested_field<T: DelegateValue>(
    value: &Value,
    def: &PropertyDef,
    constant: bool,
) -> std::result::Result<T, String> {
    let parsed = parse_property(value, def, def.id).map_err(|error| format!("{error:#}"))?;
    if constant && matches!(parsed, PropertyExpression::Expr(_)) {
        return Err(format!("`{}` does not support expressions", def.id));
    }
    // A nested field has no node of its own to name, so errors name the field.
    T::from_property_expression(parsed, NodeId::new(0), def.id).map_err(|error| match error {
        LumenError::Property(PropertyError::InvalidType {
            expected, actual, ..
        }) => format!("`{}` expected {expected}, got {actual}", def.id),
        error => error.to_string(),
    })
}

/// Like [`parse_nested_field`] for `Option` and `Override` fields.
pub(crate) fn parse_nested_optional_field<T: OptionalDelegate>(
    value: &Value,
    def: &PropertyDef,
    constant: bool,
) -> std::result::Result<T, String> {
    if value.is_null() && T::NULLABLE {
        return Ok(T::from_override(Override::Clear));
    }
    parse_nested_field(value, def, constant).map(|value| T::from_override(Override::Set(value)))
}

/// The JSON that parses to `property`, naming enum values from `enum_def`.
pub(crate) fn property_to_json(property: &PropertyExpression, enum_def: Option<&EnumDef>) -> Value {
    let value = match property {
        PropertyExpression::Expr(expr) => return Value::from(format!("={}", expr.source)),
        PropertyExpression::Value(value) => value,
    };
    match value {
        PropertyValue::Float(value) => Value::from(*value),
        PropertyValue::Int(value) => enum_def
            .and_then(|def| def.options.iter().find(|option| option.value == *value))
            .map_or_else(|| Value::from(*value), |option| Value::from(option.name)),
        PropertyValue::Bool(value) => Value::from(*value),
        PropertyValue::String(text) if text.starts_with('=') => Value::from(format!("={text}")),
        PropertyValue::String(text) => Value::from(text.as_str()),
        PropertyValue::Color(color) => Value::from(color.to_vec()),
        PropertyValue::Paint(paint) => paint.to_json_value(),
        PropertyValue::Vec2((x, y)) => Value::from(vec![*x, *y]),
        PropertyValue::ShaderUniforms(uniforms) => uniforms.to_json_value(),
        PropertyValue::ClipProps(props) => props.to_json_value(),
        PropertyValue::TextContent(content) => content.to_json_value(),
        PropertyValue::TextLine(line) => Value::Object(line.to_json()),
        PropertyValue::TextHighlight(highlight) => Value::Object(highlight.to_json()),
        PropertyValue::FontFeatures(features) => features.to_json_value(),
        PropertyValue::TextStroke(stroke) => Value::Object(stroke.to_json()),
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

/// The error for a param a node does not have.
pub fn unknown_param_error(key: &str, id: NodeId) -> anyhow::Error {
    anyhow!("unknown param `{key}` on node {id}")
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

    fn float_def() -> PropertyDef {
        def_of::<crate::node::processing::exposure::ExposureParamsDelegate>("contrast")
    }

    fn string_def() -> PropertyDef {
        def_of::<crate::node::source::media_in::MediaInParamsDelegate>("source")
    }

    fn def_of<P: crate::node::NodeParams>(id: &str) -> PropertyDef {
        P::property_defs()
            .into_iter()
            .find(|def| def.id == id)
            .unwrap()
    }

    #[test]
    fn typed_property() {
        let v = serde_json::json!(1.25);
        let p = parse_property(&v, &float_def(), "test").unwrap();
        assert!(matches!(
            p,
            PropertyExpression::Value(PropertyValue::Float(f)) if (f - 1.25).abs() < 1e-10
        ));
    }

    #[test]
    fn expression_property() {
        let v = serde_json::json!("=frame * 2");
        let p = parse_property(&v, &float_def(), "test").unwrap();
        assert!(matches!(p, PropertyExpression::Expr(_)));
    }

    #[test]
    fn leading_double_equals_is_a_literal_string() {
        for (input, expected) in [("==wow", "=wow"), ("===", "=="), ("==", "=")] {
            let p = parse_property(&serde_json::json!(input), &string_def(), "test").unwrap();
            assert!(
                matches!(&p, PropertyExpression::Value(PropertyValue::String(s)) if s == expected),
                "{input}"
            );
        }
    }
}
