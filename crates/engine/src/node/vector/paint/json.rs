#[cfg(feature = "json")]
use serde::Deserialize;
#[cfg(feature = "json")]
use serde_json::Value;

#[cfg(feature = "json")]
use super::types::{GradientPaint, GradientStop, Paint};

#[cfg(feature = "json")]
pub fn from_json_value(value: &Value) -> Result<Paint, String> {
    if let Some(color) = crate::json::parse_color(value) {
        return Ok(Paint::solid(color));
    }
    serde_path_to_error::deserialize::<_, GradientPaintJsonOwned>(value)
        .map(GradientPaint::from)
        .map(Paint::Gradient)
        .map_err(|error| format!("expected paint: {error}"))
}

#[cfg(feature = "json")]
pub fn to_json_value(paint: &Paint) -> Value {
    match paint {
        Paint::SolidColor(r, g, b, a) => Value::Array(
            [*r, *g, *b, *a]
                .iter()
                .map(|value| Value::from(*value))
                .collect(),
        ),
        Paint::Gradient(gradient) => serde_json::to_value(GradientPaintJsonRef::from(gradient))
            .expect("gradient paint serializes to JSON"),
    }
}

#[cfg(feature = "json")]
#[derive(serde::Serialize)]
struct GradientPaintJsonRef<'a> {
    #[serde(rename = "type")]
    kind: super::types::PaintKind,
    units: super::types::GradientUnits,
    spread: super::types::GradientSpread,
    interpolation: super::types::GradientInterpolation,
    start: [f32; 2],
    end: [f32; 2],
    center: [f32; 2],
    radius: [f32; 2],
    angle: f32,
    stops: &'a [GradientStop],
}

#[cfg(feature = "json")]
#[derive(serde::Deserialize)]
struct GradientPaintJsonOwned {
    #[serde(rename = "type")]
    kind: super::types::PaintKind,
    #[serde(default)]
    units: super::types::GradientUnits,
    #[serde(default)]
    spread: super::types::GradientSpread,
    #[serde(default)]
    interpolation: super::types::GradientInterpolation,
    #[serde(default)]
    start: [f32; 2],
    #[serde(default = "default_end")]
    end: [f32; 2],
    #[serde(default = "default_center")]
    center: [f32; 2],
    #[serde(default = "default_radius", deserialize_with = "deserialize_radius")]
    radius: [f32; 2],
    #[serde(default)]
    angle: f32,
    #[serde(deserialize_with = "deserialize_stops")]
    stops: Vec<GradientStop>,
}

#[cfg(feature = "json")]
impl<'a> From<&'a GradientPaint> for GradientPaintJsonRef<'a> {
    fn from(gradient: &'a GradientPaint) -> Self {
        Self {
            kind: gradient.kind,
            units: gradient.units,
            spread: gradient.spread,
            interpolation: gradient.interpolation,
            start: gradient.start,
            end: gradient.end,
            center: gradient.center,
            radius: gradient.radius,
            angle: gradient.angle,
            stops: &gradient.stops,
        }
    }
}

#[cfg(feature = "json")]
impl From<GradientPaintJsonOwned> for GradientPaint {
    fn from(value: GradientPaintJsonOwned) -> Self {
        let mut stops = value.stops;
        stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
        Self {
            kind: value.kind,
            units: value.units,
            spread: value.spread,
            interpolation: value.interpolation,
            start: value.start,
            end: value.end,
            center: value.center,
            radius: value.radius,
            angle: value.angle,
            stops,
        }
    }
}

#[cfg(feature = "json")]
fn default_end() -> [f32; 2] {
    [1.0, 0.0]
}

#[cfg(feature = "json")]
fn default_center() -> [f32; 2] {
    [0.5, 0.5]
}

#[cfg(feature = "json")]
fn default_radius() -> [f32; 2] {
    [0.5, 0.5]
}

#[cfg(feature = "json")]
fn deserialize_radius<'de, D>(deserializer: D) -> Result<[f32; 2], D::Error>
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

#[cfg(feature = "json")]
fn deserialize_stops<'de, D>(deserializer: D) -> Result<Vec<GradientStop>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    let stops = Vec::<Value>::deserialize(deserializer)?;
    if stops.is_empty() {
        return Err(D::Error::custom("`stops` must contain at least one stop"));
    }
    stops
        .iter()
        .enumerate()
        .map(|(index, value)| {
            parse_stop(value)
                .map_err(|message| D::Error::custom(format!("stop {index}: {message}")))
        })
        .collect()
}

/// Parses `[offset, color]` or `{ "offset": number, "color": color }`.
#[cfg(feature = "json")]
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
        color: crate::json::parse_color(color).ok_or("color must be a color")?,
    })
}

#[cfg(all(test, feature = "json"))]
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
}
