use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        processing::gpu_shader::{ShaderUniform, apply_runtime_shader},
        processing::raster_map::{byte_to_unit, unit_to_byte},
    },
    raster::RasterFrame,
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct HueSaturation {
    pub id: NodeId,

    #[property(expected = Float)]
    pub hue_degrees: NodeProperty,
    #[property(expected = Float)]
    pub saturation: NodeProperty,
    #[property(expected = Float)]
    pub lightness: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for HueSaturation {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            hue_degrees: NodeProperty::Float(0.0),
            saturation: NodeProperty::Float(1.0),
            lightness: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl HueSaturation {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let hue_degrees = self.resolve_hue_degrees(ctx)? as f32;
        let saturation = self.resolve_saturation(ctx)? as f32;
        let lightness = self.resolve_lightness(ctx)? as f32;
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let params = [hue_degrees / 360.0, saturation, lightness];

        apply_runtime_shader(
            source,
            HUE_SATURATION_SHADER,
            &[ShaderUniform {
                name: "params",
                values: &params,
            }],
            source.alpha_mode(),
            self.id,
            "HueSaturation",
            ctx.frame,
            ctx,
        )
    }
}

const HUE_SATURATION_SHADER: &str = r#"
uniform shader source;
uniform float params[3];

float3 rgb_to_hsl(float3 rgb) {
    float maxValue = max(max(rgb.r, rgb.g), rgb.b);
    float minValue = min(min(rgb.r, rgb.g), rgb.b);
    float lightness = (maxValue + minValue) * 0.5;
    float delta = maxValue - minValue;
    if (delta <= 0.000001) {
        return float3(0.0, 0.0, lightness);
    }

    float saturation = lightness > 0.5
        ? delta / (2.0 - maxValue - minValue)
        : delta / (maxValue + minValue);
    float hue;
    if (abs(maxValue - rgb.r) <= 0.000001) {
        hue = ((rgb.g - rgb.b) / delta + (rgb.g < rgb.b ? 6.0 : 0.0)) / 6.0;
    } else if (abs(maxValue - rgb.g) <= 0.000001) {
        hue = ((rgb.b - rgb.r) / delta + 2.0) / 6.0;
    } else {
        hue = ((rgb.r - rgb.g) / delta + 4.0) / 6.0;
    }
    return float3(hue, saturation, lightness);
}

float hue_to_rgb(float p, float q, float t) {
    t = fract(t);
    if (t < 1.0 / 6.0) {
        return p + (q - p) * 6.0 * t;
    }
    if (t < 1.0 / 2.0) {
        return q;
    }
    if (t < 2.0 / 3.0) {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    return p;
}

float3 hsl_to_rgb(float3 hsl) {
    if (hsl.y <= 0.000001) {
        return float3(hsl.z);
    }

    float q = hsl.z < 0.5 ? hsl.z * (1.0 + hsl.y) : hsl.z + hsl.y - hsl.z * hsl.y;
    float p = 2.0 * hsl.z - q;
    return float3(
        hue_to_rgb(p, q, hsl.x + 1.0 / 3.0),
        hue_to_rgb(p, q, hsl.x),
        hue_to_rgb(p, q, hsl.x - 1.0 / 3.0)
    );
}

half4 main(float2 coord) {
    half4 color = source.eval(coord);
    float3 hsl = rgb_to_hsl(float3(color.rgb));
    hsl.x = fract(hsl.x + params[0]);
    hsl.y = clamp(hsl.y * params[1], 0.0, 1.0);
    hsl.z = clamp(hsl.z + params[2], 0.0, 1.0);
    return half4(half3(hsl_to_rgb(hsl)), color.a);
}
"#;

pub(crate) fn apply_hue_saturation_bytes(
    pixels: &mut [u8],
    hue_degrees: f32,
    saturation: f32,
    lightness: f32,
) {
    let hue_offset = hue_degrees / 360.0;
    for pixel in pixels.chunks_exact_mut(4) {
        let (mut h, mut s, mut l) = rgb_to_hsl(
            byte_to_unit(pixel[0]),
            byte_to_unit(pixel[1]),
            byte_to_unit(pixel[2]),
        );
        h = (h + hue_offset).rem_euclid(1.0);
        s = (s * saturation).clamp(0.0, 1.0);
        l = (l + lightness).clamp(0.0, 1.0);
        let (r, g, b) = hsl_to_rgb(h, s, l);
        pixel[0] = unit_to_byte(r);
        pixel[1] = unit_to_byte(g);
        pixel[2] = unit_to_byte(b);
    }
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;

    if (max - min).abs() <= f32::EPSILON {
        return (0.0, 0.0, l);
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() <= f32::EPSILON {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };

    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s <= f32::EPSILON {
        return (l, l, l);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    (
        hue_to_rgb(p, q, h + 1.0 / 3.0),
        hue_to_rgb(p, q, h),
        hue_to_rgb(p, q, h - 1.0 / 3.0),
    )
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 1.0 / 2.0 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node::processing::test_support, raster::AlphaMode};

    #[test]
    fn hue_saturation_changes_hue_and_saturation() {
        let mut hue_pixels = vec![255, 0, 0, 255];
        apply_hue_saturation_bytes(&mut hue_pixels, 120.0, 1.0, 0.0);
        assert_eq!(hue_pixels, vec![0, 255, 0, 255]);

        let mut saturation_pixels = vec![64, 128, 192, 255];
        apply_hue_saturation_bytes(&mut saturation_pixels, 0.0, 0.0, 0.0);
        assert_eq!(saturation_pixels, vec![128, 128, 128, 255]);
    }

    #[test]
    fn hue_saturation_sksl_rotates_hue_and_preserves_alpha() {
        let source = test_support::frame_from_pixel([255, 0, 0, 170], AlphaMode::Premultiplied);
        let params = [120.0 / 360.0, 1.0, 0.0];

        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_runtime_shader(
                &source,
                HUE_SATURATION_SHADER,
                &[ShaderUniform {
                    name: "params",
                    values: &params,
                }],
                source.alpha_mode(),
                NodeId::new(1),
                "HueSaturation",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        assert_eq!(test_support::read_first_pixel(&output), [0, 255, 0, 170]);
    }

    #[test]
    fn hue_saturation_sksl_can_desaturate() {
        let source = test_support::frame_from_pixel([64, 128, 192, 255], AlphaMode::Premultiplied);
        let params = [0.0, 0.0, 0.0];

        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_runtime_shader(
                &source,
                HUE_SATURATION_SHADER,
                &[ShaderUniform {
                    name: "params",
                    values: &params,
                }],
                source.alpha_mode(),
                NodeId::new(1),
                "HueSaturation",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        assert_eq!(
            test_support::read_first_pixel(&output),
            [128, 128, 128, 255]
        );
    }
}
