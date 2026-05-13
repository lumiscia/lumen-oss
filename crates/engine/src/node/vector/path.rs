use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
};
use crate::node::{NodeId, NodeProperty, PortRef};

/// Produces a rasterized vector path source.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "path", name = "Path", category = "vector")]
pub struct Path {
    pub id: NodeId,
    /// SVG-style path data.
    #[property(
        kind = "string",
        name = "Path data",
        format = "path_data",
        multiline,
        recommended_rows = 5
    )]
    pub data: NodeProperty,
    /// Path origin in pixels.
    #[property(kind = "vec2")]
    pub position: NodeProperty,
    /// Enables fill rendering.
    #[property(kind = "bool")]
    pub fill_enabled: NodeProperty,
    /// Fill color.
    #[property(kind = "color")]
    pub fill_color: NodeProperty,
    /// Enables stroke rendering.
    #[property(kind = "bool")]
    pub stroke_enabled: NodeProperty,
    /// Stroke color.
    #[property(kind = "color")]
    pub stroke_color: NodeProperty,
    /// Stroke width in pixels.
    #[property(kind = "float", min = 0, step = 0.5)]
    pub stroke_width: NodeProperty,
}

impl Default for Path {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            data: NodeProperty::String("M 0 0 L 100 0 L 100 100 L 0 100 Z".to_string()),
            position: NodeProperty::Vec2((0.0, 0.0)),
            fill_enabled: NodeProperty::Bool(true),
            fill_color: NodeProperty::Color([255, 255, 255, 255]),
            stroke_enabled: NodeProperty::Bool(false),
            stroke_color: NodeProperty::Color([0, 0, 0, 255]),
            stroke_width: NodeProperty::Float(1.0),
        }
    }
}

impl GpuCompileNode for Path {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        crate::node::vector::renderer::VectorRenderer::new(ctx).compile_path(self, port)
    }
}

impl GpuFrameBindNode for Path {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Path {
            node_id,
            data,
            position,
            fill_enabled,
            fill_color,
            stroke_enabled,
            stroke_color,
            stroke_width,
            params_buffer,
            points_buffer,
            max_points,
        } = binding
        else {
            return Ok(());
        };

        let path_data =
            data.resolve_string(*node_id, "data", &ctx.expr_context(*node_id, "data"))?;
        let points = parse_path_points(&path_data, *max_points);
        let (x, y) = position.resolve_vec2(
            *node_id,
            "position",
            &ctx.expr_context(*node_id, "position"),
        )?;
        let fill = fill_color.resolve_color(
            *node_id,
            "fill_color",
            &ctx.expr_context(*node_id, "fill_color"),
        )?;
        let stroke = stroke_color.resolve_color(
            *node_id,
            "stroke_color",
            &ctx.expr_context(*node_id, "stroke_color"),
        )?;
        let mut flags = 0;
        if fill_enabled.resolve_bool(
            *node_id,
            "fill_enabled",
            &ctx.expr_context(*node_id, "fill_enabled"),
        )? {
            flags |= 1;
        }
        if stroke_enabled.resolve_bool(
            *node_id,
            "stroke_enabled",
            &ctx.expr_context(*node_id, "stroke_enabled"),
        )? {
            flags |= 2;
        }

        let params = super::renderer::PathParams {
            fill_color: rgba8_to_f32(fill),
            stroke_color: rgba8_to_f32(stroke),
            position: [x as f32, y as f32],
            stroke_width: stroke_width.resolve_float(
                *node_id,
                "stroke_width",
                &ctx.expr_context(*node_id, "stroke_width"),
            )? as f32,
            flags,
            point_count: points.len() as u32,
            _pad: [0; 3],
        };
        bound.write_buffer(*params_buffer, 0, bytemuck::bytes_of(&params));
        if !points.is_empty() {
            bound.write_buffer(*points_buffer, 0, bytemuck::cast_slice(&points));
        }
        Ok(())
    }
}

// Pragmatic fallback: this accepts SVG-like path strings by flattening every
// numeric coordinate pair into a polygon. Curves/arcs are treated as straight
// point chains until a full path tessellator lands here.
fn parse_path_points(data: &str, max_points: usize) -> Vec<super::renderer::PathPoint> {
    let mut numbers = Vec::new();
    let mut token = String::new();
    let mut previous = '\0';

    for ch in data.chars() {
        let starts_exponent = matches!(previous, 'e' | 'E') && matches!(ch, '+' | '-');
        if matches!(ch, '-' | '+') && !starts_exponent {
            if !token.is_empty() {
                if let Ok(value) = token.parse::<f32>() {
                    numbers.push(value);
                }
                token.clear();
            }
            token.push(ch);
        } else if ch.is_ascii_digit() || ch == '.' || starts_exponent || matches!(ch, 'e' | 'E') {
            token.push(ch);
        } else if !token.is_empty() {
            if let Ok(value) = token.parse::<f32>() {
                numbers.push(value);
            }
            token.clear();
        }
        previous = ch;
    }
    if !token.is_empty()
        && let Ok(value) = token.parse::<f32>()
    {
        numbers.push(value);
    }

    numbers
        .chunks_exact(2)
        .take(max_points)
        .map(|pair| super::renderer::PathPoint {
            position: [pair[0], pair[1]],
        })
        .collect()
}

fn rgba8_to_f32(color: [u8; 4]) -> [f32; 4] {
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    ]
}
