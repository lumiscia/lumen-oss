use serde::Deserialize;
use serde_json::Value;

use super::types::{GradientPaint, GradientStop, MAX_GRADIENT_STOPS, Paint};

pub fn from_json_value(value: &Value) -> Result<Paint, String> {
    if !value.is_object() {
        return crate::json::try_parse_color(value)
            .map(Paint::solid)
            .map_err(|error| format!("expected paint: {error}"));
    }
    if value.get("type").is_none() {
        return Err("expected paint: missing field `type`".to_string());
    }
    let gradient: GradientPaint = serde_path_to_error::deserialize(value)
        .map_err(|error| format!("expected paint: {error}"))?;
    if gradient.stops.is_empty() {
        return Err("expected paint: `stops` must contain at least one stop".to_string());
    }
    Ok(Paint::Gradient(gradient))
}

pub fn to_json_value(paint: &Paint) -> Value {
    match paint {
        Paint::SolidColor(r, g, b, a) => Value::Array(
            [*r, *g, *b, *a]
                .iter()
                .map(|value| Value::from(*value))
                .collect(),
        ),
        Paint::Gradient(gradient) => {
            serde_json::to_value(gradient).expect("gradient paint serializes to JSON")
        }
    }
}

pub(super) fn deserialize_radius<'de, D>(deserializer: D) -> Result<[f32; 2], D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Radius {
        Uniform(f32),
        Axes([f32; 2]),
    }

    match Radius::deserialize(deserializer) {
        Ok(Radius::Uniform(radius)) => Ok([radius, radius]),
        Ok(Radius::Axes(radius)) => Ok(radius),
        Err(_) => Err(serde::de::Error::custom("expected a number or [x, y]")),
    }
}

pub(super) fn deserialize_stops<'de, D>(deserializer: D) -> Result<Vec<GradientStop>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    let stops = Vec::<Value>::deserialize(deserializer)?;
    if stops.len() > MAX_GRADIENT_STOPS {
        return Err(D::Error::custom(format!(
            "{} stops, but a gradient takes at most {MAX_GRADIENT_STOPS}",
            stops.len()
        )));
    }
    let mut stops = stops
        .iter()
        .enumerate()
        .map(|(index, value)| {
            parse_stop(value)
                .map_err(|message| D::Error::custom(format!("stop {index}: {message}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    Ok(stops)
}

/// Parses `[offset, color]` or `{ "offset": number, "color": color }`.
fn parse_stop(value: &Value) -> Result<GradientStop, String> {
    let (offset, color) = match value {
        Value::Array(array) => match array.as_slice() {
            [offset, color] => (offset, color),
            _ => {
                return Err(format!(
                    "expected [offset, color], got array of length {}",
                    array.len()
                ));
            }
        },
        Value::Object(object) => {
            if let Some(key) = object
                .keys()
                .find(|key| !matches!(key.as_str(), "offset" | "color"))
            {
                return Err(format!("unknown field `{key}`"));
            }
            (
                object.get("offset").ok_or("missing field `offset`")?,
                object.get("color").ok_or("missing field `color`")?,
            )
        }
        _ => return Err("expected [offset, color] or { offset, color }".to_string()),
    };
    Ok(GradientStop {
        offset: offset.as_f64().ok_or("offset must be a number")? as f32,
        color: crate::json::try_parse_color(color).map_err(|error| format!("color: {error}"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_array_and_object_stops() {
        let paint = from_json_value(&serde_json::json!({
            "type": "linear",
            "stops": [[1.0, [0, 0, 255]], { "offset": 0.0, "color": "#ff0000" }]
        }))
        .unwrap();
        let Paint::Gradient(gradient) = paint else {
            panic!("expected gradient");
        };
        assert_eq!(gradient.stops[0].color, [255, 0, 0, 255]);
        assert_eq!(gradient.stops[1].color, [0, 0, 255, 255]);
    }

    #[test]
    fn rejects_malformed_stops() {
        for stop in [
            serde_json::json!([0.0, [255, 0, 0], "extra"]),
            serde_json::json!([0.0]),
            serde_json::json!([0.0, [256, 0, 0]]),
            serde_json::json!({ "offset": 0.0, "color": [255, 0, 0], "extra": 1 }),
            serde_json::json!({ "color": [255, 0, 0] }),
            serde_json::json!(0.5),
        ] {
            let error = from_json_value(&serde_json::json!({ "type": "linear", "stops": [stop] }))
                .unwrap_err();
            assert!(error.contains("stop 0"), "{stop}: {error}");
        }
    }

    #[test]
    fn rejects_empty_stop_list() {
        let error =
            from_json_value(&serde_json::json!({ "type": "linear", "stops": [] })).unwrap_err();
        assert!(error.contains("at least one stop"), "{error}");
    }

    #[test]
    fn rejects_more_stops_than_the_shader_holds() {
        let stops = |count: usize| {
            (0..count)
                .map(|index| serde_json::json!([index as f32 / 10.0, [0, 0, 0]]))
                .collect::<Vec<_>>()
        };
        from_json_value(&serde_json::json!({ "type": "linear", "stops": stops(8) })).unwrap();
        let error = from_json_value(&serde_json::json!({ "type": "linear", "stops": stops(9) }))
            .unwrap_err();
        assert!(
            error.contains("9 stops") && error.contains("at most 8"),
            "{error}"
        );
    }

    #[test]
    fn rejects_unknown_gradient_fields_and_missing_type_or_stops() {
        let stops = serde_json::json!([[0.0, [0, 0, 0]]]);
        for (paint, expected) in [
            (
                serde_json::json!({ "type": "linear", "stops": stops, "colour": 1 }),
                "colour",
            ),
            (serde_json::json!({ "stops": stops }), "type"),
            (serde_json::json!({ "type": "linear" }), "at least one stop"),
        ] {
            let error = from_json_value(&paint).unwrap_err();
            assert!(error.contains(expected), "{paint}: {error}");
        }
    }
}
