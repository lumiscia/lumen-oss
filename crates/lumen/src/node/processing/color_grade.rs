use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        processing::gpu_shader::{ChildShader, ShaderUniform, apply_runtime_shader_with_children},
    },
    raster::{AlphaMode, RasterFrame, make_skia_image},
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};
use skia_safe::{SamplingOptions, TileMode};

pub const IDENTITY_LUT: &str = "identity";
/// Inline 1D RGB LUT format: `rgb1d: r,g,b; r,g,b; ...`.
/// Components may be normalized `0.0..1.0` or byte-like `0..255`.
pub const LUT_FORMAT_NAME: &str = "rgb1d";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum LutInterpolation {
    Nearest = 0,
    Linear = 1,
}

impl LutInterpolation {
    fn from_int(value: i64) -> Self {
        match value {
            0 => Self::Nearest,
            _ => Self::Linear,
        }
    }
}

#[derive(Debug, Clone, Node)]
pub struct ColorGrade {
    pub id: NodeId,

    #[property(expected = String)]
    pub lut_source: NodeProperty,
    #[property(expected = Float)]
    pub strength: NodeProperty,
    #[property(expected = Int)]
    pub interpolation: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            lut_source: NodeProperty::String(IDENTITY_LUT.to_string()),
            strength: NodeProperty::Float(1.0),
            interpolation: NodeProperty::Int(LutInterpolation::Linear as i64),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl ColorGrade {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let lut_source = self.resolve_lut_source(ctx)?;
        let strength = self.resolve_strength(ctx)? as f32;
        let interpolation = LutInterpolation::from_int(self.resolve_interpolation(ctx)?);

        apply_color_grade(
            source,
            &lut_source,
            strength,
            interpolation,
            self.id,
            ctx.frame,
            ctx,
        )
    }
}

pub fn apply_color_grade<S: SurfacePool, M: crate::media::MediaStore>(
    source: &RasterFrame,
    lut_source: &str,
    strength: f32,
    interpolation: LutInterpolation,
    node_id: NodeId,
    frame: u32,
    ctx: &mut RenderContext<'_, S, M>,
) -> crate::Result<RasterFrame> {
    let lut = Lut1d::parse(lut_source, node_id, frame)?;
    let strength = strength.clamp(0.0, 1.0);

    if lut.is_identity() || strength <= 0.0 {
        return source.snapshot();
    }

    let lut_image = lut.shader_image(interpolation, node_id, frame)?;
    let lut_shader = lut_image
        .to_shader(
            Some((TileMode::Clamp, TileMode::Clamp)),
            SamplingOptions::default(),
            None,
        )
        .ok_or_else(|| lut_error(node_id, frame, "LUT shader creation failed"))?;
    let strength = [strength];
    apply_runtime_shader_with_children(
        source,
        COLOR_GRADE_SHADER,
        &[ShaderUniform {
            name: "strength",
            values: &strength,
        }],
        &[ChildShader {
            name: "lut",
            shader: lut_shader,
        }],
        source.alpha_mode(),
        node_id,
        "ColorGrade",
        frame,
        ctx,
    )
}

const LUT_TABLE_SIZE: usize = 256;

const COLOR_GRADE_SHADER: &str = r#"
uniform shader source;
uniform shader lut;
uniform float strength;

float3 sample_lut(float3 color) {
    return float3(
        lut.eval(float2(clamp(color.r, 0.0, 1.0) * 255.0, 0.0)).r,
        lut.eval(float2(clamp(color.g, 0.0, 1.0) * 255.0, 0.0)).g,
        lut.eval(float2(clamp(color.b, 0.0, 1.0) * 255.0, 0.0)).b
    );
}

half4 main(float2 coord) {
    half4 color = source.eval(coord);
    float3 original = float3(color.rgb);
    float3 graded = sample_lut(original);
    return half4(half3(mix(original, graded, clamp(strength, 0.0, 1.0))), color.a);
}
"#;

#[derive(Debug, Clone)]
struct Lut1d {
    stops: Vec<[f32; 3]>,
}

impl Lut1d {
    fn parse(source: &str, node_id: NodeId, frame: u32) -> crate::Result<Self> {
        let source = source.trim();
        if source.is_empty() || source.eq_ignore_ascii_case(IDENTITY_LUT) {
            return Ok(Self {
                stops: vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            });
        }

        let source = source
            .strip_prefix(LUT_FORMAT_NAME)
            .and_then(|rest| rest.strip_prefix(':'))
            .unwrap_or(source);
        let mut stops = Vec::new();
        for triplet in source.split(';') {
            let triplet = triplet.trim();
            if triplet.is_empty() {
                continue;
            }

            let components = triplet
                .split([',', ' ', '\t'])
                .filter(|part| !part.is_empty())
                .map(str::parse::<f32>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| lut_error(node_id, frame, "LUT contains a non-numeric component"))?;

            if components.len() != 3 {
                return Err(lut_error(
                    node_id,
                    frame,
                    format!("LUT triplet `{triplet}` must contain exactly three RGB components"),
                ));
            }

            stops.push([
                normalize_component(components[0]),
                normalize_component(components[1]),
                normalize_component(components[2]),
            ]);
        }

        if stops.len() < 2 {
            return Err(lut_error(
                node_id,
                frame,
                "LUT must contain at least two RGB triplets",
            ));
        }

        Ok(Self { stops })
    }

    fn is_identity(&self) -> bool {
        self.stops == [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]
    }

    fn map(&self, color: [f32; 3], interpolation: LutInterpolation) -> [f32; 3] {
        [
            self.map_channel(color[0], 0, interpolation),
            self.map_channel(color[1], 1, interpolation),
            self.map_channel(color[2], 2, interpolation),
        ]
    }

    fn shader_image(
        &self,
        interpolation: LutInterpolation,
        node_id: NodeId,
        frame: u32,
    ) -> crate::Result<skia_safe::Image> {
        let mut pixels = [0; LUT_TABLE_SIZE * 4];
        for index in 0..LUT_TABLE_SIZE {
            let value = index as f32 / (LUT_TABLE_SIZE - 1) as f32;
            let offset = index * 4;
            pixels[offset] = normalized_to_u8(self.map_channel(value, 0, interpolation));
            pixels[offset + 1] = normalized_to_u8(self.map_channel(value, 1, interpolation));
            pixels[offset + 2] = normalized_to_u8(self.map_channel(value, 2, interpolation));
            pixels[offset + 3] = 255;
        }

        make_skia_image(
            &pixels,
            LUT_TABLE_SIZE as u32,
            1,
            LUT_TABLE_SIZE * 4,
            AlphaMode::Unpremultiplied,
        )
        .ok_or_else(|| lut_error(node_id, frame, "LUT image creation failed"))
    }

    fn map_channel(&self, value: f32, channel: usize, interpolation: LutInterpolation) -> f32 {
        let value = value.clamp(0.0, 1.0);
        let scaled = value * (self.stops.len() - 1) as f32;
        match interpolation {
            LutInterpolation::Nearest => {
                let index = scaled.round() as usize;
                self.stops[index.min(self.stops.len() - 1)][channel]
            }
            LutInterpolation::Linear => {
                let low = scaled.floor() as usize;
                let high = (low + 1).min(self.stops.len() - 1);
                let t = scaled - low as f32;
                let a = self.stops[low][channel];
                let b = self.stops[high][channel];
                a + (b - a) * t
            }
        }
    }
}

fn normalize_component(value: f32) -> f32 {
    if value > 1.0 {
        (value / 255.0).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn lut_error(node_id: NodeId, frame: u32, details: impl Into<String>) -> crate::error::LumenError {
    crate::error::RenderError::NodeEvaluation {
        frame,
        node_id,
        node_kind: "ColorGrade",
        details: details.into(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        node::processing::test_support,
        raster::{AlphaMode, RectI},
    };

    fn frame_from_pixel(pixel: [u8; 4]) -> RasterFrame {
        RasterFrame::from_rgba_bytes(
            &pixel,
            1,
            1,
            4,
            AlphaMode::Unpremultiplied,
            RectI::from_size(1, 1),
            RectI::from_size(1, 1),
        )
        .expect("test frame")
    }

    fn read_first_pixel(frame: &RasterFrame) -> [u8; 4] {
        let mut pixel = [0; 4];
        frame.read_pixels_into(&mut pixel, 4).expect("read pixel");
        pixel
    }

    #[test]
    fn color_grade_sksl_lut_maps_known_color_with_linear_strength() {
        let source = frame_from_pixel([128, 64, 255, 200]);
        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_color_grade(
                &source,
                "rgb1d: 0,0,0; 255,128,0",
                0.5,
                LutInterpolation::Linear,
                NodeId::new(10),
                0,
                ctx,
            )
        })
        .expect("graded output");

        assert_eq!(output.format_rect(), source.format_rect());
        assert_eq!(output.data_rect(), source.data_rect());
        assert_eq!(read_first_pixel(&output), [128, 48, 128, 200]);
    }

    #[test]
    fn color_grade_sksl_lut_supports_nearest_interpolation() {
        let source = frame_from_pixel([80, 190, 120, 255]);
        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_color_grade(
                &source,
                "0,0,255; 255,0,0",
                1.0,
                LutInterpolation::Nearest,
                NodeId::new(11),
                0,
                ctx,
            )
        })
        .expect("graded output");

        assert_eq!(read_first_pixel(&output), [0, 0, 255, 255]);
    }
}
fn normalized_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
