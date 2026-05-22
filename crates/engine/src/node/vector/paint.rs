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
    pub(crate) offsets: [[f32; 4]; MAX_GRADIENT_STOPS],
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
        paint.offsets[0][0] = 0.0;
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
        paint.offsets[index][0] = stop.offset.clamp(0.0, 1.0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn gpu_paint_size_matches_wgsl_uniform_stride() {
        // The GpuPaint struct is uploaded as a uniform buffer. Each offset element
        // must have 16-byte stride to satisfy WGSL uniform address space rules.
        let size = std::mem::size_of::<GpuPaint>();
        // colors: 8 × 16 = 128
        // offsets: 8 × 16 = 128 (padded from 32)
        // start: 8, end: 8, center: 8, radius: 8
        // angle: 4, kind: 4, units: 4, spread: 4, interpolation: 4
        // stop_count: 4, _pad: 8
        // Total: 128 + 128 + 8 * 4 + 4 * 5 + 4 + 8
        // = 256 + 32 + 20 + 4 + 8 = 320
        assert_eq!(size, 320);
    }

    #[test]
    fn gpu_paint_solid_zeroed_is_well_formed() {
        let paint = GpuPaint::solid([255, 0, 0, 255]);
        assert_eq!(paint.colors[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(paint.offsets[0][0], 0.0);
        assert_eq!(paint.stop_count, 1);
        // Remainder should be zero-initialised.
        let zeroed = GpuPaint::zeroed();
        for i in 1..MAX_GRADIENT_STOPS {
            assert_eq!(paint.colors[i], zeroed.colors[i]);
            assert_eq!(paint.offsets[i], zeroed.offsets[i]);
        }
        assert_eq!(paint.kind, 0);
        assert_eq!(paint.units, 0);
        assert_eq!(paint.spread, 0);
        assert_eq!(paint.interpolation, 0);
    }

    #[test]
    fn gpu_paint_gradient_places_offsets_in_first_component() {
        let gradient = GradientPaint {
            kind: PaintKind::LinearGradient,
            units: GradientUnits::UserSpace,
            spread: GradientSpread::Repeat,
            interpolation: GradientInterpolation::LinearSrgb,
            start: [10.0, 20.0],
            end: [100.0, 200.0],
            center: [50.0, 60.0],
            radius: [30.0, 40.0],
            angle: 45.0,
            stops: vec![
                GradientStop {
                    offset: 0.25,
                    color: [255, 0, 0, 255],
                },
                GradientStop {
                    offset: 0.75,
                    color: [0, 0, 255, 255],
                },
            ],
        };
        let gpu = gradient_to_gpu(&gradient, [0, 0, 0, 255]);
        assert_eq!(gpu.kind, 1); // LinearGradient → 1
        assert_eq!(gpu.units, 1); // UserSpace → 1
        assert_eq!(gpu.spread, 1); // Repeat → 1
        assert_eq!(gpu.interpolation, 1); // LinearSrgb → 1
        assert_eq!(gpu.start, [10.0, 20.0]);
        assert_eq!(gpu.end, [100.0, 200.0]);
        assert_eq!(gpu.center, [50.0, 60.0]);
        assert_eq!(gpu.radius, [30.0, 40.0]);
        assert_eq!(gpu.angle, 45.0);
        assert_eq!(gpu.stop_count, 2);
        // Offsets stored in first component of each vec4 slot.
        assert!((gpu.offsets[0][0] - 0.25).abs() < 0.001);
        assert!((gpu.offsets[1][0] - 0.75).abs() < 0.001);
        // Colors match.
        assert!((gpu.colors[0][0] - 1.0).abs() < 0.01);
        assert!((gpu.colors[0][2] - 0.0).abs() < 0.01);
        assert!((gpu.colors[1][2] - 1.0).abs() < 0.01);
    }

    #[test]
    fn gpu_paint_gradient_clamps_offset_to_0_1() {
        let gradient = GradientPaint {
            stops: vec![
                GradientStop {
                    offset: -0.5,
                    color: [128, 128, 128, 255],
                },
                GradientStop {
                    offset: 1.5,
                    color: [64, 64, 64, 255],
                },
            ],
            ..Default::default()
        };
        let gpu = gradient_to_gpu(&gradient, [0, 0, 0, 255]);
        assert!((gpu.offsets[0][0] - 0.0).abs() < 0.001);
        assert!((gpu.offsets[1][0] - 1.0).abs() < 0.001);
    }

    #[test]
    fn gpu_paint_gradient_falls_back_on_empty_stops() {
        let gradient = GradientPaint {
            stops: Vec::new(),
            ..Default::default()
        };
        let gpu = gradient_to_gpu(&gradient, [64, 128, 192, 255]);
        assert_eq!(gpu.stop_count, 1);
        assert!((gpu.colors[0][0] - 0.25).abs() < 0.02);
        assert!((gpu.colors[0][1] - 0.5).abs() < 0.02);
        assert!((gpu.colors[0][2] - 0.75).abs() < 0.02);
        assert!((gpu.colors[0][3] - 1.0).abs() < 0.01);
    }

    #[cfg(feature = "json")]
    #[test]
    fn paint_json_roundtrips_solid_color() {
        let solid = Paint::solid([64, 128, 192, 255]);
        let json = solid.to_json_value();
        let parsed = Paint::from_json_value(&json).unwrap();
        assert_eq!(parsed, solid);
    }

    #[cfg(feature = "json")]
    #[test]
    fn paint_json_roundtrips_gradient() {
        let gradient = Paint::Gradient(GradientPaint {
            kind: PaintKind::RadialGradient,
            units: GradientUnits::UserSpace,
            spread: GradientSpread::Reflect,
            interpolation: GradientInterpolation::LinearSrgb,
            start: [0.1, 0.2],
            end: [0.8, 0.9],
            center: [0.4, 0.5],
            radius: [0.3, 0.3],
            angle: 90.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: [255, 0, 0, 255],
                },
                GradientStop {
                    offset: 1.0,
                    color: [0, 255, 0, 255],
                },
            ],
        });
        let json = gradient.to_json_value();
        let parsed = Paint::from_json_value(&json).unwrap();
        assert_eq!(parsed, gradient);
    }

    #[cfg(feature = "json")]
    #[test]
    fn paint_parses_solid_color_array() {
        let json = serde_json::json!([100, 150, 200, 255]);
        let paint = Paint::from_json_value(&json).unwrap();
        assert_eq!(paint, Paint::solid([100, 150, 200, 255]));
    }

    #[cfg(feature = "json")]
    #[test]
    fn paint_parses_gradient_object() {
        let json = serde_json::json!({
            "type": "linear_gradient",
            "units": "user_space",
            "spread": "repeat",
            "interpolation": "linear_srgb",
            "start": [0.0, 0.0],
            "end": [1.0, 0.0],
            "stops": [
                {"offset": 0.0, "color": [255, 0, 0, 255]},
                {"offset": 1.0, "color": [0, 0, 255, 255]}
            ]
        });
        let paint = Paint::from_json_value(&json).unwrap();
        match paint {
            Paint::Gradient(g) => {
                assert_eq!(g.kind, PaintKind::LinearGradient);
                assert_eq!(g.units, GradientUnits::UserSpace);
                assert_eq!(g.spread, GradientSpread::Repeat);
                assert_eq!(g.interpolation, GradientInterpolation::LinearSrgb);
                assert_eq!(g.stops.len(), 2);
                assert_eq!(g.stops[0].color, [255, 0, 0, 255]);
                assert_eq!(g.stops[1].color, [0, 0, 255, 255]);
            }
            _ => panic!("expected Gradient"),
        }
    }

    #[cfg(feature = "json")]
    #[test]
    fn paint_parses_gradient_with_shorthand_type_names() {
        for (json, expected) in [
            (
                serde_json::json!({"type": "linear", "stops": [[0.0, [255, 0, 0, 255]], [1.0, [0, 0, 255, 255]]]}),
                PaintKind::LinearGradient,
            ),
            (
                serde_json::json!({"type": "radial", "stops": [[0.0, [255, 0, 0, 255]], [1.0, [0, 0, 255, 255]]]}),
                PaintKind::RadialGradient,
            ),
            (
                serde_json::json!({"type": "conic", "stops": [[0.0, [255, 0, 0, 255]], [1.0, [0, 0, 255, 255]]]}),
                PaintKind::ConicGradient,
            ),
        ] {
            let paint = Paint::from_json_value(&json).unwrap();
            match paint {
                Paint::Gradient(g) => assert_eq!(g.kind, expected),
                _ => panic!("expected Gradient"),
            }
        }
    }

    #[cfg(feature = "json")]
    #[test]
    fn paint_parses_gradient_stops_as_flat_arrays() {
        let json = serde_json::json!({
            "type": "linear_gradient",
            "stops": [
                [0.0, [255, 0, 0, 255]],
                [0.5, [0, 255, 0, 128]],
                [1.0, [0, 0, 255, 64]]
            ]
        });
        let paint = Paint::from_json_value(&json).unwrap();
        match paint {
            Paint::Gradient(g) => {
                assert_eq!(g.stops.len(), 3);
                assert_eq!(g.stops[0].offset, 0.0);
                assert_eq!(g.stops[0].color, [255, 0, 0, 255]);
                assert_eq!(g.stops[1].offset, 0.5);
                assert_eq!(g.stops[1].color, [0, 255, 0, 128]);
                assert_eq!(g.stops[2].offset, 1.0);
                assert_eq!(g.stops[2].color, [0, 0, 255, 64]);
            }
            _ => panic!("expected Gradient"),
        }
    }

    #[test]
    fn gpu_paint_uses_wgsl_compatible_gradient_kind_mappings() {
        assert_eq!(
            gradient_to_gpu(
                &GradientPaint {
                    kind: PaintKind::LinearGradient,
                    ..Default::default()
                },
                [0, 0, 0, 255],
            )
            .kind,
            1
        );
        assert_eq!(
            gradient_to_gpu(
                &GradientPaint {
                    kind: PaintKind::RadialGradient,
                    ..Default::default()
                },
                [0, 0, 0, 255],
            )
            .kind,
            2
        );
        assert_eq!(
            gradient_to_gpu(
                &GradientPaint {
                    kind: PaintKind::ConicGradient,
                    ..Default::default()
                },
                [0, 0, 0, 255],
            )
            .kind,
            3
        );
    }

    #[test]
    fn gpu_paint_uses_wgsl_compatible_spread_mappings() {
        assert_eq!(
            gradient_to_gpu(
                &GradientPaint {
                    spread: GradientSpread::Pad,
                    ..Default::default()
                },
                [0, 0, 0, 255],
            )
            .spread,
            0
        );
        assert_eq!(
            gradient_to_gpu(
                &GradientPaint {
                    spread: GradientSpread::Repeat,
                    ..Default::default()
                },
                [0, 0, 0, 255],
            )
            .spread,
            1
        );
        assert_eq!(
            gradient_to_gpu(
                &GradientPaint {
                    spread: GradientSpread::Reflect,
                    ..Default::default()
                },
                [0, 0, 0, 255],
            )
            .spread,
            2
        );
    }

    #[test]
    fn gpu_paint_truncates_stops_beyond_max() {
        let many_stops: Vec<GradientStop> = (0..(MAX_GRADIENT_STOPS + 3))
            .map(|i| GradientStop {
                offset: i as f32 / 10.0,
                color: [i as u8, 0, 0, 255],
            })
            .collect();
        let gradient = GradientPaint {
            stops: many_stops,
            ..Default::default()
        };
        let gpu = gradient_to_gpu(&gradient, [0, 0, 0, 255]);
        assert_eq!(gpu.stop_count, MAX_GRADIENT_STOPS as u32);
        // First stop color.
        assert!((gpu.colors[0][0] - 0.0).abs() < 0.01);
        // Last accepted stop color.
        let last_idx = MAX_GRADIENT_STOPS - 1;
        assert!((gpu.colors[last_idx][0] - (last_idx as f32 / 255.0)).abs() < 0.01);
    }

    #[test]
    fn paint_to_gpu_solid_uses_color_from_enum() {
        let paint = Paint::solid([50, 100, 150, 200]);
        let gpu = paint.to_gpu([0, 0, 0, 255]);
        assert_eq!(gpu.kind, 0);
        assert_eq!(gpu.stop_count, 1);
        assert!((gpu.colors[0][0] - (50.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][1] - (100.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][2] - (150.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][3] - (200.0 / 255.0)).abs() < 0.01);
    }

    #[test]
    fn paint_to_gpu_gradient_uses_fallback_when_no_stops() {
        let paint = Paint::Gradient(GradientPaint {
            stops: Vec::new(),
            ..Default::default()
        });
        let gpu = paint.to_gpu([10, 20, 30, 40]);
        assert_eq!(gpu.stop_count, 1);
        assert!((gpu.colors[0][0] - (10.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][1] - (20.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][2] - (30.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][3] - (40.0 / 255.0)).abs() < 0.01);
    }

    #[test]
    fn paint_delegate_roundtrips_solid_color() {
        let solid = Paint::SolidColor(10, 20, 30, 40);
        let delegate = PaintDelegate::from(solid.clone());
        let roundtripped = delegate.into_evaluated();
        assert_eq!(roundtripped, solid);
    }

    #[test]
    fn paint_delegate_roundtrips_complete_gradient() {
        let gradient = Paint::Gradient(GradientPaint {
            kind: PaintKind::RadialGradient,
            units: GradientUnits::UserSpace,
            spread: GradientSpread::Reflect,
            interpolation: GradientInterpolation::LinearSrgb,
            start: [5.0, 10.0],
            end: [15.0, 20.0],
            center: [25.0, 30.0],
            radius: [35.0, 40.0],
            angle: 45.0,
            stops: vec![
                GradientStop {
                    offset: 0.2,
                    color: [10, 20, 30, 40],
                },
                GradientStop {
                    offset: 0.8,
                    color: [50, 60, 70, 80],
                },
            ],
        });
        let delegate = PaintDelegate::from(gradient.clone());
        let roundtripped = delegate.into_evaluated();
        assert_eq!(roundtripped, gradient);
    }

    #[test]
    fn paint_default_is_solid_black() {
        assert_eq!(Paint::default(), Paint::solid([0, 0, 0, 255]));
    }

    #[test]
    fn gradient_paint_default_is_linear_object_bounding_box() {
        let g = GradientPaint::default();
        assert_eq!(g.kind, PaintKind::LinearGradient);
        assert_eq!(g.units, GradientUnits::ObjectBoundingBox);
        assert_eq!(g.spread, GradientSpread::Pad);
        assert_eq!(g.interpolation, GradientInterpolation::Srgb);
        assert_eq!(g.start, [0.0, 0.0]);
        assert_eq!(g.end, [1.0, 0.0]);
        assert_eq!(g.center, [0.5, 0.5]);
        assert_eq!(g.radius, [0.5, 0.5]);
        assert_eq!(g.angle, 0.0);
        assert!(g.stops.is_empty());
    }
}
