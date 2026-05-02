use crate::{
    gpu_image::GpuImageFrame,
    node::{
        NodeId, NodeProperty, PortRef,
        processing::gpu_shader::{ShaderUniform, apply_runtime_shader},
    },
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct Levels {
    pub id: NodeId,

    #[property(expected = Float)]
    pub black_point: NodeProperty,
    #[property(expected = Float)]
    pub white_point: NodeProperty,
    #[property(expected = Float)]
    pub gamma: NodeProperty,
    #[property(expected = Float)]
    pub output_black: NodeProperty,
    #[property(expected = Float)]
    pub output_white: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            black_point: NodeProperty::Float(0.0),
            white_point: NodeProperty::Float(1.0),
            gamma: NodeProperty::Float(1.0),
            output_black: NodeProperty::Float(0.0),
            output_white: NodeProperty::Float(1.0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Levels {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let black_point = self.resolve_black_point(ctx)? as f32;
        let white_point = self.resolve_white_point(ctx)? as f32;
        let gamma = self.resolve_gamma(ctx)? as f32;
        let output_black = self.resolve_output_black(ctx)? as f32;
        let output_white = self.resolve_output_white(ctx)? as f32;
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let params = [black_point, white_point, gamma, output_black, output_white];

        apply_runtime_shader(
            source,
            LEVELS_SHADER,
            &[ShaderUniform {
                name: "params",
                values: &params,
            }],
            source.alpha_mode(),
            self.id,
            "Levels",
            ctx.frame,
            ctx,
        )
    }
}

const LEVELS_SHADER: &str = r#"
uniform shader source;
uniform float params[5];

half4 main(float2 coord) {
    half4 color = source.eval(coord);
    float black = clamp(params[0], 0.0, 1.0);
    float white = clamp(params[1], 0.0, 1.0);
    if (white <= black) {
        white = min(black + 0.000001, 1.0);
    }
    float gamma = max(params[2], 0.0001);
    float outBlack = clamp(params[3], 0.0, 1.0);
    float outWhite = clamp(params[4], 0.0, 1.0);
    float3 normalized = clamp((float3(color.rgb) - black) / (white - black), 0.0, 1.0);
    normalized = pow(normalized, float3(1.0 / gamma));
    return half4(half3(outBlack + normalized * (outWhite - outBlack)), color.a);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gpu_image::AlphaMode, node::processing::test_support};

    #[test]
    fn levels_sksl_remaps_black_white_gamma_and_output_range() {
        let source = test_support::frame_from_pixel([64, 128, 192, 210], AlphaMode::Premultiplied);
        let params = [64.0 / 255.0, 192.0 / 255.0, 2.0, 0.0, 1.0];

        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_runtime_shader(
                &source,
                LEVELS_SHADER,
                &[ShaderUniform {
                    name: "params",
                    values: &params,
                }],
                source.alpha_mode(),
                NodeId::new(1),
                "Levels",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        assert_eq!(test_support::read_first_pixel(&output), [0, 180, 255, 210]);
    }
}
