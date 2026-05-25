use serde_json::Value;

use super::types::{
    GradientInterpolation, GradientPaint, GradientSpread, GradientStop, GradientUnits, Paint,
    PaintKind,
};

#[cfg(feature = "json")]
pub fn from_json_value(value: &Value) -> Option<Paint> {
    if let Some(color) = crate::json::parse_color(value) {
        return Some(Paint::solid(color));
    }
    parse_gradient(value).map(Paint::Gradient)
}

pub fn to_json_value(paint: &Paint) -> Value {
    match paint {
        Paint::SolidColor(r, g, b, a) => Value::Array(
            [*r, *g, *b, *a]
                .iter()
                .map(|value| Value::from(*value))
                .collect(),
        ),
        Paint::Gradient(gradient) => gradient_to_json_value(gradient),
    }
}

#[cfg(feature = "json")]
fn parse_gradient(value: &Value) -> Option<GradientPaint> {
    let object = value.as_object()?;

    let kind = match string_field(object, "type")? {
        "linear_gradient" | "linear" => PaintKind::LinearGradient,
        "radial_gradient" | "radial" => PaintKind::RadialGradient,
        "conic_gradient" | "conic" => PaintKind::ConicGradient,
        _ => return None,
    };
    let units = match string_field(object, "units").unwrap_or("object_bounding_box") {
        "user_space" | "userSpaceOnUse" => GradientUnits::UserSpace,
        _ => GradientUnits::ObjectBoundingBox,
    };
    let spread = match string_field(object, "spread").unwrap_or("pad") {
        "repeat" => GradientSpread::Repeat,
        "reflect" => GradientSpread::Reflect,
        _ => GradientSpread::Pad,
    };
    let interpolation = match string_field(object, "interpolation").unwrap_or("srgb") {
        "linear_srgb" | "linear" => GradientInterpolation::LinearSrgb,
        _ => GradientInterpolation::Srgb,
    };
    let start = vec2_field(object, "start", [0.0, 0.0]);
    let end = vec2_field(object, "end", [1.0, 0.0]);
    let center = vec2_field(object, "center", [0.5, 0.5]);
    let radius = number_field(object, "radius", 0.5) as f32;
    let radius = vec2_field(object, "radius", [radius, radius]);
    let angle = number_field(object, "angle", 0.0) as f32;
    let mut stops = object
        .get("stops")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(parse_stop)
        .collect::<Vec<_>>();
    stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    Some(GradientPaint {
        kind,
        units,
        spread,
        interpolation,
        start,
        end,
        center,
        radius,
        angle,
        stops,
    })
}

fn gradient_to_json_value(gradient: &GradientPaint) -> Value {
    let kind = match gradient.kind {
        PaintKind::LinearGradient => "linear_gradient",
        PaintKind::RadialGradient => "radial_gradient",
        PaintKind::ConicGradient => "conic_gradient",
    };
    let units = match gradient.units {
        GradientUnits::ObjectBoundingBox => "object_bounding_box",
        GradientUnits::UserSpace => "user_space",
    };
    let spread = match gradient.spread {
        GradientSpread::Pad => "pad",
        GradientSpread::Repeat => "repeat",
        GradientSpread::Reflect => "reflect",
    };
    let interpolation = match gradient.interpolation {
        GradientInterpolation::Srgb => "srgb",
        GradientInterpolation::LinearSrgb => "linear_srgb",
    };
    serde_json::json!({
        "type": kind,
        "units": units,
        "spread": spread,
        "interpolation": interpolation,
        "start": gradient.start,
        "end": gradient.end,
        "center": gradient.center,
        "radius": gradient.radius,
        "angle": gradient.angle,
        "stops": gradient.stops.iter().map(|stop| {
            serde_json::json!({ "offset": stop.offset, "color": stop.color })
        }).collect::<Vec<_>>(),
    })
}

#[cfg(feature = "json")]
fn parse_stop(value: &Value) -> Option<GradientStop> {
    if let Some(array) = value.as_array()
        && array.len() >= 2
    {
        return Some(GradientStop {
            offset: array[0].as_f64()? as f32,
            color: crate::json::parse_color(&array[1])?,
        });
    }
    let object = value.as_object()?;
    Some(GradientStop {
        offset: object.get("offset")?.as_f64()? as f32,
        color: crate::json::parse_color(object.get("color")?)?,
    })
}

#[cfg(feature = "json")]
fn string_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

#[cfg(feature = "json")]
fn number_field(object: &serde_json::Map<String, Value>, key: &str, fallback: f64) -> f64 {
    object.get(key).and_then(Value::as_f64).unwrap_or(fallback)
}

#[cfg(feature = "json")]
fn vec2_field(object: &serde_json::Map<String, Value>, key: &str, fallback: [f32; 2]) -> [f32; 2] {
    let Some(array) = object.get(key).and_then(Value::as_array) else {
        return fallback;
    };
    if array.len() != 2 {
        return fallback;
    }
    [
        array[0].as_f64().unwrap_or(f64::from(fallback[0])) as f32,
        array[1].as_f64().unwrap_or(f64::from(fallback[1])) as f32,
    ]
}
