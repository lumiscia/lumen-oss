use bytemuck::Zeroable;
use serde_json::Value;

pub(crate) const MAX_GRADIENT_STOPS: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::Delegate)]
pub enum PaintKind {
    #[default]
    LinearGradient,
    RadialGradient,
    ConicGradient,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::Delegate)]
pub enum GradientUnits {
    #[default]
    ObjectBoundingBox,
    UserSpace,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::Delegate)]
pub enum GradientSpread {
    #[default]
    Pad,
    Repeat,
    Reflect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::Delegate)]
pub enum GradientInterpolation {
    #[default]
    Srgb,
    LinearSrgb,
}

#[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
pub struct GradientStop {
    #[meta(min = 0, max = 1, step = 0.01)]
    pub offset: f32,
    #[meta()]
    pub color: [u8; 4],
}

#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
pub struct GradientPaint {
    #[meta()]
    pub kind: PaintKind,
    #[meta()]
    pub units: GradientUnits,
    #[meta()]
    pub spread: GradientSpread,
    #[meta()]
    pub interpolation: GradientInterpolation,
    #[meta()]
    pub start: [f32; 2],
    #[meta()]
    pub end: [f32; 2],
    #[meta()]
    pub center: [f32; 2],
    #[meta()]
    pub radius: [f32; 2],
    #[meta(step = 1)]
    pub angle: f32,
    #[meta()]
    pub stops: Vec<GradientStop>,
}

#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
#[delegate(kind = "paint")]
pub enum Paint {
    SolidColor(u8, u8, u8, u8),
    Gradient(GradientPaint),
}

impl Paint {
    pub fn solid(color: [u8; 4]) -> Self {
        Self::SolidColor(color[0], color[1], color[2], color[3])
    }

    #[cfg(feature = "json")]
    pub fn from_json_value(value: &Value) -> Option<Self> {
        if let Some(color) = crate::json::parse_color(value) {
            return Some(Self::solid(color));
        }
        parse_gradient(value).map(Self::Gradient)
    }

    pub(crate) fn to_gpu(&self, fallback: [u8; 4]) -> GpuPaint {
        let delegate = PaintDelegate::from(self.clone());
        match delegate.into_evaluated() {
            Self::SolidColor(r, g, b, a) => GpuPaint::solid([r, g, b, a]),
            Self::Gradient(gradient) => gradient_to_gpu(&gradient, fallback),
        }
    }

    pub fn to_json_value(&self) -> Value {
        match self {
            Self::SolidColor(r, g, b, a) => Value::Array(
                [*r, *g, *b, *a]
                    .iter()
                    .map(|value| Value::from(*value))
                    .collect(),
            ),
            Self::Gradient(gradient) => gradient_to_json_value(gradient),
        }
    }
}

impl Default for Paint {
    fn default() -> Self {
        Self::solid([0, 0, 0, 255])
    }
}

impl Default for GradientPaint {
    fn default() -> Self {
        Self {
            kind: PaintKind::LinearGradient,
            units: GradientUnits::ObjectBoundingBox,
            spread: GradientSpread::Pad,
            interpolation: GradientInterpolation::Srgb,
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            center: [0.5, 0.5],
            radius: [0.5, 0.5],
            angle: 0.0,
            stops: Vec::new(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GpuPaint {
    pub(crate) colors: [[f32; 4]; MAX_GRADIENT_STOPS],
    pub(crate) offsets: [f32; MAX_GRADIENT_STOPS],
    pub(crate) start: [f32; 2],
    pub(crate) end: [f32; 2],
    pub(crate) center: [f32; 2],
    pub(crate) radius: [f32; 2],
    pub(crate) angle: f32,
    pub(crate) kind: u32,
    pub(crate) units: u32,
    pub(crate) spread: u32,
    pub(crate) interpolation: u32,
    pub(crate) stop_count: u32,
    pub(crate) _pad: [u32; 2],
}

impl GpuPaint {
    pub(crate) fn solid(color: [u8; 4]) -> Self {
        let mut paint = Self::zeroed();
        paint.colors[0] = rgba8_to_f32(color);
        paint.offsets[0] = 0.0;
        paint.stop_count = 1;
        paint
    }
}

#[cfg(feature = "json")]
fn parse_gradient(value: &Value) -> Option<GradientPaint> {
    let Some(object) = value.as_object() else {
        return None;
    };

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

fn gradient_to_gpu(gradient: &GradientPaint, fallback: [u8; 4]) -> GpuPaint {
    let mut paint = GpuPaint::solid(fallback);
    paint.kind = match gradient.kind {
        PaintKind::LinearGradient => 1,
        PaintKind::RadialGradient => 2,
        PaintKind::ConicGradient => 3,
    };
    paint.units = match gradient.units {
        GradientUnits::ObjectBoundingBox => 0,
        GradientUnits::UserSpace => 1,
    };
    paint.spread = match gradient.spread {
        GradientSpread::Pad => 0,
        GradientSpread::Repeat => 1,
        GradientSpread::Reflect => 2,
    };
    paint.interpolation = match gradient.interpolation {
        GradientInterpolation::Srgb => 0,
        GradientInterpolation::LinearSrgb => 1,
    };
    paint.start = gradient.start;
    paint.end = gradient.end;
    paint.center = gradient.center;
    paint.radius = gradient.radius;
    paint.angle = gradient.angle;
    for (index, stop) in gradient.stops.iter().take(MAX_GRADIENT_STOPS).enumerate() {
        paint.offsets[index] = stop.offset.clamp(0.0, 1.0);
        paint.colors[index] = rgba8_to_f32(stop.color);
        paint.stop_count = index as u32 + 1;
    }
    paint
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

fn rgba8_to_f32(color: [u8; 4]) -> [f32; 4] {
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    ]
}
