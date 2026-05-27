mod gpu;
mod json;
mod types;

pub(crate) use gpu::GpuPaint;

pub use types::{
    GradientInterpolation, GradientPaint, GradientSpread, GradientStop, GradientUnits, Paint,
    PaintDelegate, PaintKind,
};

impl Paint {
    pub fn solid(color: [u8; 4]) -> Self {
        Self::SolidColor(color[0], color[1], color[2], color[3])
    }

    #[cfg(feature = "json")]
    pub fn from_json_value(value: &serde_json::Value) -> Option<Self> {
        json::from_json_value(value)
    }

    pub(crate) fn to_gpu(&self, fallback: [u8; 4]) -> gpu::GpuPaint {
        match self {
            Self::SolidColor(r, g, b, a) => gpu::GpuPaint::solid([*r, *g, *b, *a]),
            Self::Gradient(gradient) => gpu::gradient_to_gpu(gradient, fallback),
        }
    }

    #[cfg(feature = "json")]
    pub fn to_json_value(&self) -> serde_json::Value {
        json::to_json_value(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;
    use gpu::{GpuPaint, test_gradient_to_gpu as gradient_to_gpu};
    use types::{MAX_GRADIENT_STOPS, PaintDelegate};

    #[test]
    fn gpu_paint_size_matches_wgsl_uniform_stride() {
        let size = std::mem::size_of::<GpuPaint>();
        assert_eq!(size, 320);
    }

    #[test]
    fn gpu_paint_solid_zeroed_is_well_formed() {
        let paint = GpuPaint::solid([255, 0, 0, 255]);
        assert_eq!(paint.colors[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(paint.offsets[0][0], 0.0);
        assert_eq!(paint.stop_count, 1);
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
        assert!((gpu.colors[0][0] - 0.0).abs() < 0.01);
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
        let roundtripped = delegate.into_evaluated().unwrap();
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
        let roundtripped = delegate.into_evaluated().unwrap();
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

    #[test]
    fn conic_gradient_shaders_wrap_angle_offsets() {
        for shader in [
            crate::node::source::background::SHADER,
            crate::node::vector::renderer::SHAPE_SHADER,
            crate::node::vector::renderer::PATH_SHADER,
        ] {
            assert!(shader.contains("fract(angle - paint.angle / 360.0)"));
        }
    }
}
