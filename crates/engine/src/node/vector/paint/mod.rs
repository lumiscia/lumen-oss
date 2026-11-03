mod gpu;
mod json;
mod types;

pub(crate) use gpu::GpuPaint;

pub use types::{
    GradientInterpolation, GradientPaint, GradientPaintDelegate, GradientSpread, GradientStop,
    GradientStopDelegate, GradientUnits, MAX_GRADIENT_STOPS, Paint, PaintDelegate, PaintKind,
};

impl Paint {
    pub fn solid(color: [u8; 4]) -> Self {
        Self::SolidColor(color[0], color[1], color[2], color[3])
    }

    pub fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        json::from_json_value(value)
    }

    pub(crate) fn to_gpu(&self) -> gpu::GpuPaint {
        match self {
            Self::SolidColor(r, g, b, a) => gpu::GpuPaint::solid([*r, *g, *b, *a]),
            Self::Gradient(gradient) => gpu::gradient_to_gpu(gradient),
        }
    }

    pub fn to_json_value(&self) -> serde_json::Value {
        json::to_json_value(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;
    use gpu::{GpuPaint, gradient_to_gpu};

    #[test]
    fn gpu_paint_size_matches_wgsl_uniform_stride() {
        let size = std::mem::size_of::<GpuPaint>();
        assert_eq!(size, 320);
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
        let gpu = gradient_to_gpu(&gradient);
        assert_eq!(gpu.kind, 1);
        assert_eq!(gpu.units, 1);
        assert_eq!(gpu.spread, 1);
        assert_eq!(gpu.interpolation, 1);
        assert_eq!(gpu.start, [10.0, 20.0]);
        assert_eq!(gpu.end, [100.0, 200.0]);
        assert_eq!(gpu.center, [50.0, 60.0]);
        assert_eq!(gpu.radius, [30.0, 40.0]);
        assert_eq!(gpu.angle, 45.0);
        assert_eq!(gpu.stop_count, 2);
        assert!((gpu.offsets[0][0] - 0.25).abs() < 0.001);
        assert!((gpu.offsets[1][0] - 0.75).abs() < 0.001);
        assert!((gpu.colors[0][0] - 1.0).abs() < 0.01);
        assert!((gpu.colors[0][2] - 0.0).abs() < 0.01);
        assert!((gpu.colors[1][2] - 1.0).abs() < 0.01);

        let out_of_range = GradientPaint {
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
        let gpu = gradient_to_gpu(&out_of_range);
        assert!((gpu.offsets[0][0] - 0.0).abs() < 0.001);
        assert!((gpu.offsets[1][0] - 1.0).abs() < 0.001);
    }

    #[test]
    fn paint_json_parses_the_public_forms_and_roundtrips() {
        let red_to_blue = vec![
            GradientStop {
                offset: 0.0,
                color: [255, 0, 0, 255],
            },
            GradientStop {
                offset: 1.0,
                color: [0, 0, 255, 255],
            },
        ];
        for (json, expected) in [
            (
                serde_json::json!([100, 150, 200, 255]),
                Paint::solid([100, 150, 200, 255]),
            ),
            (
                serde_json::json!({
                    "type": "radial_gradient",
                    "units": "user_space",
                    "spread": "reflect",
                    "interpolation": "linear_srgb",
                    "center": [0.4, 0.5],
                    "radius": [0.25, 0.5],
                    "start": [0.1, 0.2],
                    "end": [0.8, 0.9],
                    "angle": 90.0,
                    "stops": [
                        {"offset": 0.0, "color": [255, 0, 0, 255]},
                        {"offset": 1.0, "color": [0, 0, 255, 255]}
                    ]
                }),
                Paint::Gradient(GradientPaint {
                    kind: PaintKind::RadialGradient,
                    units: GradientUnits::UserSpace,
                    spread: GradientSpread::Reflect,
                    interpolation: GradientInterpolation::LinearSrgb,
                    start: [0.1, 0.2],
                    end: [0.8, 0.9],
                    center: [0.4, 0.5],
                    radius: [0.25, 0.5],
                    angle: 90.0,
                    stops: red_to_blue,
                }),
            ),
            (
                serde_json::json!({
                    "type": "linear_gradient",
                    "spread": "repeat",
                    "stops": [
                        [0.0, [255, 0, 0, 255]],
                        [0.5, [0, 255, 0, 128]],
                        [1.0, [0, 0, 255, 64]]
                    ]
                }),
                Paint::Gradient(GradientPaint {
                    spread: GradientSpread::Repeat,
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: [255, 0, 0, 255],
                        },
                        GradientStop {
                            offset: 0.5,
                            color: [0, 255, 0, 128],
                        },
                        GradientStop {
                            offset: 1.0,
                            color: [0, 0, 255, 64],
                        },
                    ],
                    ..Default::default()
                }),
            ),
        ] {
            let parsed = Paint::from_json_value(&json).unwrap();
            assert_eq!(parsed, expected, "{json}");
            assert_eq!(
                Paint::from_json_value(&parsed.to_json_value()).unwrap(),
                parsed
            );
        }
    }

    #[test]
    fn paint_parses_gradient_shorthand_type_names() {
        for (name, kind) in [
            ("linear", PaintKind::LinearGradient),
            ("radial", PaintKind::RadialGradient),
            ("conic", PaintKind::ConicGradient),
        ] {
            let json = serde_json::json!({
                "type": name,
                "stops": [[0.0, [255, 0, 0, 255]], [1.0, [0, 0, 255, 255]]]
            });
            let Paint::Gradient(gradient) = Paint::from_json_value(&json).unwrap() else {
                panic!("{name}: expected Gradient");
            };
            assert_eq!(gradient.kind, kind, "{name}");
        }
    }

    #[test]
    fn paint_parses_scalar_gradient_radius() {
        let json = serde_json::json!({
            "type": "radial_gradient",
            "radius": 0.25,
            "stops": [[0.0, [255, 0, 0, 255]], [1.0, [0, 0, 255, 255]]]
        });
        let Paint::Gradient(gradient) = Paint::from_json_value(&json).unwrap() else {
            panic!("expected Gradient");
        };
        assert_eq!(gradient.radius, [0.25, 0.25]);

        let json = serde_json::json!({
            "type": "radial_gradient",
            "radius": "big",
            "stops": []
        });
        let error = Paint::from_json_value(&json).unwrap_err();
        assert!(error.contains("radius"), "{error}");
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
        let gpu = gradient_to_gpu(&gradient);
        assert_eq!(gpu.stop_count, MAX_GRADIENT_STOPS as u32);
        assert!((gpu.colors[0][0] - 0.0).abs() < 0.01);
        let last_idx = MAX_GRADIENT_STOPS - 1;
        assert!((gpu.colors[last_idx][0] - (last_idx as f32 / 255.0)).abs() < 0.01);
    }

    #[test]
    fn paint_to_gpu_solid_uses_color_from_enum() {
        let paint = Paint::solid([50, 100, 150, 200]);
        let gpu = paint.to_gpu();
        assert_eq!(gpu.kind, 0);
        assert_eq!(gpu.stop_count, 1);
        assert!((gpu.colors[0][0] - (50.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][1] - (100.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][2] - (150.0 / 255.0)).abs() < 0.01);
        assert!((gpu.colors[0][3] - (200.0 / 255.0)).abs() < 0.01);
        let zeroed = GpuPaint::zeroed();
        for i in 1..MAX_GRADIENT_STOPS {
            assert_eq!(gpu.colors[i], zeroed.colors[i]);
            assert_eq!(gpu.offsets[i], zeroed.offsets[i]);
        }
        assert_eq!((gpu.units, gpu.spread, gpu.interpolation), (0, 0, 0));
    }

    #[test]
    fn paint_to_gpu_gradient_without_stops_is_transparent() {
        let gpu = Paint::Gradient(GradientPaint::default()).to_gpu();
        assert_eq!(gpu.stop_count, 1);
        assert_eq!(gpu.colors[0], [0.0; 4]);
    }
}
