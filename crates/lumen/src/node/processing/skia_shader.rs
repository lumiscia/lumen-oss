use crate::{
    gpu_image::GpuImageFrame,
    media::MediaStore,
    node::{
        NodeId, NodeProperty, PortRef,
        processing::gpu_shader::{ShaderUniform, apply_runtime_shader},
    },
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};

pub const SOURCE_CHILD_NAME: &str = "source";
pub const INPUT_CHILD_NAME: &str = "input";
pub const UNIFORMS_FLOAT4_NAME: &str = "uniforms";

/// Default SkSL contract:
/// declare `uniform shader source;` (or `input`) and optional float uniforms
/// `uniform0`..`uniform3`, `uniforms` as float4, and `resolution` as float2.
pub const DEFAULT_SHADER_SOURCE: &str = r#"
uniform shader source;
uniform float uniform0;
uniform float uniform1;
uniform float uniform2;
uniform float uniform3;

half4 main(float2 coord) {
    half4 color = source.eval(coord);
    return half4(color.rgb * half3(uniform0, uniform1, uniform2), color.a * half(uniform3));
}
"#;

#[derive(Debug, Clone, Node)]
pub struct SkiaShader {
    pub id: NodeId,

    #[property(expected = String)]
    pub shader_source: NodeProperty,
    #[property(expected = Float)]
    pub uniform0: NodeProperty,
    #[property(expected = Float)]
    pub uniform1: NodeProperty,
    #[property(expected = Float)]
    pub uniform2: NodeProperty,
    #[property(expected = Float)]
    pub uniform3: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for SkiaShader {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            shader_source: NodeProperty::String(DEFAULT_SHADER_SOURCE.to_string()),
            uniform0: NodeProperty::Float(1.0),
            uniform1: NodeProperty::Float(1.0),
            uniform2: NodeProperty::Float(1.0),
            uniform3: NodeProperty::Float(1.0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl SkiaShader {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let shader_source = self.resolve_shader_source(ctx)?;
        let uniforms = [
            self.resolve_uniform0(ctx)? as f32,
            self.resolve_uniform1(ctx)? as f32,
            self.resolve_uniform2(ctx)? as f32,
            self.resolve_uniform3(ctx)? as f32,
        ];

        apply_skia_shader(source, &shader_source, uniforms, self.id, ctx.frame, ctx)
    }
}

pub fn apply_skia_shader<S: SurfacePool, M: MediaStore>(
    source: &GpuImageFrame,
    shader_source: &str,
    uniforms: [f32; 4],
    node_id: NodeId,
    frame: u32,
    ctx: &mut RenderContext<'_, S, M>,
) -> crate::Result<GpuImageFrame> {
    let uniform0 = [uniforms[0]];
    let uniform1 = [uniforms[1]];
    let uniform2 = [uniforms[2]];
    let uniform3 = [uniforms[3]];
    let uniform_values = [
        ShaderUniform {
            name: "uniform0",
            values: &uniform0,
        },
        ShaderUniform {
            name: "uniform1",
            values: &uniform1,
        },
        ShaderUniform {
            name: "uniform2",
            values: &uniform2,
        },
        ShaderUniform {
            name: "uniform3",
            values: &uniform3,
        },
        ShaderUniform {
            name: UNIFORMS_FLOAT4_NAME,
            values: &uniforms,
        },
    ];

    apply_runtime_shader(
        source,
        shader_source,
        &uniform_values,
        source.alpha_mode(),
        node_id,
        "SkiaShader",
        frame,
        ctx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gpu_image::AlphaMode, node::processing::test_support};

    fn frame_from_pixel(pixel: [u8; 4]) -> GpuImageFrame {
        test_support::frame_from_pixel(pixel, AlphaMode::Premultiplied)
    }

    fn read_first_pixel(frame: &GpuImageFrame) -> [u8; 4] {
        test_support::read_first_pixel(frame)
    }

    #[test]
    fn skia_shader_inverts_source_pixel_and_preserves_alpha() {
        let shader = r#"
            uniform shader source;
            uniform float uniform0;
            uniform float uniform1;
            uniform float uniform2;
            uniform float uniform3;

            half4 main(float2 coord) {
                half4 color = source.eval(coord);
                return half4((half3(1.0) - color.rgb) * half3(uniform0, uniform1, uniform2), color.a * half(uniform3));
            }
        "#;
        let source = frame_from_pixel([10, 20, 30, 255]);
        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_skia_shader(
                &source,
                shader,
                [1.0, 1.0, 1.0, 1.0],
                NodeId::new(9),
                0,
                ctx,
            )
        })
        .expect("shader output");

        assert_eq!(output.format_rect(), source.format_rect());
        assert_eq!(output.data_rect(), source.data_rect());
        assert_eq!(output.alpha_mode(), source.alpha_mode());
        assert_eq!(read_first_pixel(&output), [245, 235, 225, 255]);
    }
}
