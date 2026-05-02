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
pub struct Exposure {
    pub id: NodeId,

    #[property(expected = Float)]
    pub exposure: NodeProperty,
    #[property(expected = Float)]
    pub contrast: NodeProperty,
    #[property(expected = Float)]
    pub offset: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for Exposure {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            exposure: NodeProperty::Float(0.0),
            contrast: NodeProperty::Float(1.0),
            offset: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Exposure {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let exposure = self.resolve_exposure(ctx)? as f32;
        let contrast = self.resolve_contrast(ctx)? as f32;
        let offset = self.resolve_offset(ctx)? as f32;
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let params = [exposure, contrast, offset];

        apply_runtime_shader(
            source,
            EXPOSURE_SHADER,
            &[ShaderUniform {
                name: "params",
                values: &params,
            }],
            source.alpha_mode(),
            self.id,
            "Exposure",
            ctx.frame,
            ctx,
        )
    }
}

const EXPOSURE_SHADER: &str = r#"
uniform shader source;
uniform float params[3];

half4 main(float2 coord) {
    half4 color = source.eval(coord);
    float exposureGain = exp2(params[0]);
    float3 rgb = float3(color.rgb) * exposureGain + params[2];
    rgb = (rgb - 0.5) * params[1] + 0.5;
    return half4(half3(clamp(rgb, 0.0, 1.0)), color.a);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gpu_image::AlphaMode, node::processing::test_support};

    #[test]
    fn exposure_sksl_applies_gain_offset_and_contrast() {
        let source = test_support::frame_from_pixel([64, 128, 192, 200], AlphaMode::Premultiplied);
        let params = [1.0, 1.5, 0.0];

        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_runtime_shader(
                &source,
                EXPOSURE_SHADER,
                &[ShaderUniform {
                    name: "params",
                    values: &params,
                }],
                source.alpha_mode(),
                NodeId::new(1),
                "Exposure",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        assert_eq!(
            test_support::read_first_pixel(&output),
            [128, 255, 255, 200]
        );
    }
}
