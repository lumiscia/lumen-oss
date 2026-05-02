use crate::{
    error::RenderError,
    expr::Expression,
    gpu_image::GpuImageFrame,
    media::MediaStore,
    node::{
        NodeId, NodeProperty, PortRef,
        processing::gpu_shader::{
            DynamicShaderUniform, DynamicUniformValues, apply_runtime_shader_dynamic,
        },
    },
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};

pub const SOURCE_CHILD_NAME: &str = "source";
pub const INPUT_CHILD_NAME: &str = "input";

/// Default SkSL contract: declare `uniform shader source;` (or `input`).
/// Numeric uniforms declared by the shader can be supplied in `uniforms`.
pub const DEFAULT_SHADER_SOURCE: &str = r#"
uniform shader source;

half4 main(float2 coord) {
    return source.eval(coord);
}
"#;

#[derive(Debug, Clone, Node)]
pub struct SkiaShader {
    pub id: NodeId,

    #[property(expected = String)]
    pub shader_source: NodeProperty,
    #[property(expected = String)]
    pub uniforms: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for SkiaShader {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            shader_source: NodeProperty::String(DEFAULT_SHADER_SOURCE.to_string()),
            uniforms: NodeProperty::String(String::new()),
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
        let uniforms = self.resolve_uniforms(ctx)?;
        let uniforms = parse_uniforms_payload(&uniforms, self.id, ctx.frame, ctx)?;

        apply_skia_shader(source, &shader_source, &uniforms, self.id, ctx.frame, ctx)
    }
}

pub(crate) fn apply_skia_shader<S: SurfacePool, M: MediaStore>(
    source: &GpuImageFrame,
    shader_source: &str,
    uniforms: &[DynamicShaderUniform],
    node_id: NodeId,
    frame: u32,
    ctx: &mut RenderContext<'_, S, M>,
) -> crate::Result<GpuImageFrame> {
    apply_runtime_shader_dynamic(
        source,
        shader_source,
        uniforms,
        source.alpha_mode(),
        node_id,
        "SkiaShader",
        frame,
        ctx,
    )
}

fn parse_uniforms_payload<S: SurfacePool, M: MediaStore>(
    payload: &str,
    node_id: NodeId,
    frame: u32,
    ctx: &RenderContext<'_, S, M>,
) -> crate::Result<Vec<DynamicShaderUniform>> {
    let mut uniforms = Vec::new();

    for (line_index, raw_line) in payload.lines().enumerate() {
        let line = raw_line
            .split_once('#')
            .map_or(raw_line, |(value, _)| value)
            .trim();
        if line.is_empty() {
            continue;
        }

        let (name, values) = line.split_once('=').ok_or_else(|| {
            skia_shader_error(
                node_id,
                frame,
                format!("uniform line {} must use `name = value`", line_index + 1),
            )
        })?;
        let name = name.trim();
        if name.is_empty() {
            return Err(skia_shader_error(
                node_id,
                frame,
                format!("uniform line {} has an empty name", line_index + 1),
            ));
        }

        let mut parsed_values = Vec::new();
        for (value_index, raw_value) in values.split(',').enumerate() {
            let value = raw_value.trim();
            if value.is_empty() {
                return Err(skia_shader_error(
                    node_id,
                    frame,
                    format!("uniform `{name}` value {} is empty", value_index + 1),
                ));
            }
            parsed_values.push(resolve_uniform_value(
                value,
                node_id,
                frame,
                name,
                value_index,
                ctx,
            )?);
        }

        uniforms.push(DynamicShaderUniform {
            name: name.to_string(),
            values: DynamicUniformValues::Number(parsed_values),
        });
    }

    Ok(uniforms)
}

fn resolve_uniform_value<S: SurfacePool, M: MediaStore>(
    value: &str,
    node_id: NodeId,
    frame: u32,
    name: &str,
    index: usize,
    ctx: &RenderContext<'_, S, M>,
) -> crate::Result<f64> {
    if let Some(expression) = value.strip_prefix('=') {
        let expression = Expression::parse(expression).map_err(|details| {
            skia_shader_error(
                node_id,
                frame,
                format!("uniform `{name}` expression parse failed: {details}"),
            )
        })?;
        return expression
            .evaluate(&ctx.expr_context(format!("{}.uniforms.{}[{}]", node_id, name, index)))
            .and_then(|value| {
                value.as_f64().ok_or_else(|| {
                    skia_shader_error(
                        node_id,
                        frame,
                        format!("uniform `{name}` expression did not evaluate to a number"),
                    )
                })
            });
    }

    value.parse::<f64>().map_err(|_| {
        skia_shader_error(
            node_id,
            frame,
            format!("uniform `{name}` value `{value}` is not numeric"),
        )
    })
}

fn skia_shader_error(
    node_id: NodeId,
    frame: u32,
    details: impl Into<String>,
) -> crate::error::LumenError {
    RenderError::NodeEvaluation {
        frame,
        node_id,
        node_kind: "SkiaShader",
        details: details.into(),
    }
    .into()
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
            uniform float gain;
            uniform int invert;

            half4 main(float2 coord) {
                half4 color = source.eval(coord);
                half3 rgb = invert == 1 ? half3(1.0) - color.rgb : color.rgb;
                return half4(rgb * half(gain), color.a);
            }
        "#;
        let source = frame_from_pixel([10, 20, 30, 255]);
        let uniforms = vec![
            DynamicShaderUniform {
                name: "gain".to_string(),
                values: DynamicUniformValues::Number(vec![1.0]),
            },
            DynamicShaderUniform {
                name: "invert".to_string(),
                values: DynamicUniformValues::Number(vec![1.0]),
            },
        ];
        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_skia_shader(&source, shader, &uniforms, NodeId::new(9), 0, ctx)
        })
        .expect("shader output");

        assert_eq!(output.format_rect(), source.format_rect());
        assert_eq!(output.data_rect(), source.data_rect());
        assert_eq!(output.alpha_mode(), source.alpha_mode());
        assert_eq!(read_first_pixel(&output), [245, 235, 225, 255]);
    }

    #[test]
    fn uniforms_payload_supports_vectors_and_expressions() {
        let source = r#"
gain = =time + 1.0
offset = 1, 2, 3
mode = 1
"#;

        let uniforms = test_support::with_test_context(1, 1, |ctx| {
            parse_uniforms_payload(source, NodeId::new(9), 24, ctx)
        })
        .expect("uniform payload");

        assert_eq!(uniforms.len(), 3);
        assert_eq!(
            uniforms[0],
            DynamicShaderUniform {
                name: "gain".to_string(),
                values: DynamicUniformValues::Number(vec![1.0]),
            }
        );
        assert_eq!(
            uniforms[1],
            DynamicShaderUniform {
                name: "offset".to_string(),
                values: DynamicUniformValues::Number(vec![1.0, 2.0, 3.0]),
            }
        );
    }
}
