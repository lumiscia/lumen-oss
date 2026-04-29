use crate::{
    error::{LumenError, PropertyError},
    node::{
        NodeId, NodeProperty, PortRef,
        processing::gpu_shader::{ShaderUniform, apply_runtime_shader},
    },
    raster::{AlphaMode, RasterFrame},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct AlphaPremultiply {
    pub id: NodeId,

    #[property(expected = String)]
    pub mode: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for AlphaPremultiply {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            mode: NodeProperty::String("premultiply".to_string()),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl AlphaPremultiply {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let operation = parse_alpha_operation(self.id, &self.resolve_mode(ctx)?)?;
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let alpha_mode = match operation {
            AlphaOperation::Premultiply => AlphaMode::Premultiplied,
            AlphaOperation::Unpremultiply => AlphaMode::Unpremultiplied,
        };
        let operation_uniform = [operation.as_uniform()];

        apply_runtime_shader(
            source,
            ALPHA_PREMULTIPLY_SHADER,
            &[ShaderUniform {
                name: "operation",
                values: &operation_uniform,
            }],
            alpha_mode,
            self.id,
            "AlphaPremultiply",
            ctx.frame,
            ctx,
        )
    }
}

const ALPHA_PREMULTIPLY_SHADER: &str = r#"
uniform shader source;
uniform float operation;

half4 main(float2 coord) {
    return source.eval(coord);
}
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlphaOperation {
    Premultiply,
    Unpremultiply,
}

impl AlphaOperation {
    fn as_uniform(self) -> f32 {
        match self {
            Self::Premultiply => 0.0,
            Self::Unpremultiply => 1.0,
        }
    }
}

pub(crate) fn apply_alpha_operation_bytes(pixels: &mut [u8], operation: AlphaOperation) {
    for pixel in pixels.chunks_exact_mut(4) {
        match operation {
            AlphaOperation::Premultiply => {
                let alpha = u16::from(pixel[3]);
                for channel in &mut pixel[..3] {
                    *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
                }
            }
            AlphaOperation::Unpremultiply => {
                let alpha = u16::from(pixel[3]);
                if alpha == 0 {
                    pixel[..3].fill(0);
                } else {
                    for channel in &mut pixel[..3] {
                        *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                    }
                }
            }
        }
    }
}

fn parse_alpha_operation(node_id: NodeId, mode: &str) -> crate::Result<AlphaOperation> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "premultiply" | "premul" | "multiply" => Ok(AlphaOperation::Premultiply),
        "unpremultiply" | "unpremul" | "straight" | "unmultiply" => {
            Ok(AlphaOperation::Unpremultiply)
        }
        _ => Err(invalid_mode(node_id)),
    }
}

fn invalid_mode(node_id: NodeId) -> LumenError {
    PropertyError::InvalidType {
        node_id,
        property_path: "mode".to_string(),
        expected: "`premultiply` or `unpremultiply`",
        actual: "String",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node::processing::test_support, raster::AlphaMode};

    #[test]
    fn alpha_premultiply_and_unpremultiply_have_expected_rgba() {
        let mut premul = vec![100, 50, 25, 128];
        apply_alpha_operation_bytes(&mut premul, AlphaOperation::Premultiply);
        assert_eq!(premul, vec![50, 25, 13, 128]);

        let mut straight = premul;
        apply_alpha_operation_bytes(&mut straight, AlphaOperation::Unpremultiply);
        assert_eq!(straight, vec![100, 50, 26, 128]);

        let mut transparent = vec![100, 50, 25, 0];
        apply_alpha_operation_bytes(&mut transparent, AlphaOperation::Unpremultiply);
        assert_eq!(transparent, vec![0, 0, 0, 0]);
    }

    #[test]
    fn alpha_premultiply_sksl_multiplies_rgb_by_alpha() {
        let source = test_support::frame_from_pixel([100, 50, 25, 128], AlphaMode::Unpremultiplied);
        let operation = [AlphaOperation::Premultiply.as_uniform()];

        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_runtime_shader(
                &source,
                ALPHA_PREMULTIPLY_SHADER,
                &[ShaderUniform {
                    name: "operation",
                    values: &operation,
                }],
                AlphaMode::Premultiplied,
                NodeId::new(1),
                "AlphaPremultiply",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        assert_eq!(test_support::read_first_pixel(&output), [50, 25, 13, 128]);
    }

    #[test]
    fn alpha_premultiply_sksl_unmultiplies_and_clears_transparent_rgb() {
        let source = test_support::frame_from_rgba(
            &[50, 25, 13, 128, 100, 50, 25, 0],
            2,
            1,
            AlphaMode::Premultiplied,
        );
        let operation = [AlphaOperation::Unpremultiply.as_uniform()];

        let output = test_support::with_test_context(2, 1, |ctx| {
            apply_runtime_shader(
                &source,
                ALPHA_PREMULTIPLY_SHADER,
                &[ShaderUniform {
                    name: "operation",
                    values: &operation,
                }],
                AlphaMode::Unpremultiplied,
                NodeId::new(1),
                "AlphaPremultiply",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        test_support::assert_pixel_near(
            test_support::read_pixels(&output)[..4].try_into().unwrap(),
            [100, 50, 26, 128],
            1,
        );
        assert_eq!(&test_support::read_pixels(&output)[4..8], &[0, 0, 0, 0]);
    }
}
