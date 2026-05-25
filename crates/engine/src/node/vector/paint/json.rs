#[cfg(feature = "json")]
use serde::Deserialize;
use serde_json::Value;

use super::types::{GradientPaint, GradientStop, Paint};

#[cfg(feature = "json")]
pub fn from_json_value(value: &Value) -> Option<Paint> {
    if let Some(color) = crate::json::parse_color(value) {
        return Some(Paint::solid(color));
    }
    serde_json::from_value::<GradientPaintJsonOwned>(value.clone())
        .ok()
        .map(GradientPaint::from)
        .map(Paint::Gradient)
}

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
    #[serde(default = "default_radius")]
    radius: [f32; 2],
    #[serde(default)]
    angle: f32,
    #[serde(deserialize_with = "deserialize_stops")]
    stops: Vec<GradientStop>,
}

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
fn deserialize_stops<'de, D>(deserializer: D) -> Result<Vec<GradientStop>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    let values = Vec::<Value>::deserialize(deserializer)?;
    values
        .into_iter()
        .map(|value| {
            if let Some(array) = value.as_array()
                && array.len() >= 2
            {
                return Ok(GradientStop {
                    offset: array[0]
                        .as_f64()
                        .ok_or_else(|| D::Error::custom("gradient stop offset must be numeric"))?
                        as f32,
                    color: crate::json::parse_color(&array[1])
                        .ok_or_else(|| D::Error::custom("gradient stop color must be a color"))?,
                });
            }
            serde_json::from_value(value).map_err(D::Error::custom)
        })
        .collect()
}
