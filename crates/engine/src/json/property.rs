//! The JSON forms params share: a leading `=`, whole numbers and colors.

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::expr::parse_hex_color;
use crate::node::NodeId;

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

/// The error for an `=expression` where a param takes literals only.
pub(crate) fn expression_unsupported(path: &str) -> String {
    format!("`{path}` does not support expressions")
}

/// Fails when `value` is an `=expression`, for params that take literals only.
pub(crate) fn reject_expression(value: &Value, path: &str) -> std::result::Result<(), String> {
    match value.as_str().and_then(classify_string) {
        Some(StringParam::Expression(_)) => Err(expression_unsupported(path)),
        _ => Ok(()),
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
    parse_hex_color(hex).ok_or_else(|| format!("hex color `{text}` contains a non-hex digit"))
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
}
