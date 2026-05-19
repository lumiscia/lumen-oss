use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};
use crate::node::{Deferred, NodeId, NodeParams, PortRef};

/// Produces a rasterized vector path source.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedPathParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct PathParams {
    /// SVG-style path data.
    #[param(
        kind = "string",
        name = "Path data",
        format = "path_data",
        multiline,
        recommended_rows = 5
    )]
    pub data: Deferred<String>,
    /// Path origin in pixels.
    #[param(kind = "vec2")]
    pub position: Deferred<(f64, f64)>,
    /// Enables fill rendering.
    #[param(kind = "bool")]
    pub fill_enabled: Deferred<bool>,
    /// Fill color.
    #[param(kind = "color")]
    pub fill_color: Deferred<[u8; 4]>,
    /// Enables stroke rendering.
    #[param(kind = "bool")]
    pub stroke_enabled: Deferred<bool>,
    /// Stroke color.
    #[param(kind = "color")]
    pub stroke_color: Deferred<[u8; 4]>,
    /// Stroke width in pixels.
    #[param(kind = "float", min = 0, step = 0.5)]
    pub stroke_width: Deferred<f64>,
}

impl Default for PathParams {
    fn default() -> Self {
        Self {
            data: Deferred::value("M 0 0 L 100 0 L 100 100 L 0 100 Z".to_string()),
            position: Deferred::value((0.0, 0.0)),
            fill_enabled: Deferred::value(true),
            fill_color: Deferred::value([255, 255, 255, 255]),
            stroke_enabled: Deferred::value(false),
            stroke_color: Deferred::value([0, 0, 0, 255]),
            stroke_width: Deferred::value(1.0),
        }
    }
}

/// Produces a rasterized vector path source.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "path", name = "Path", category = "vector")]
pub struct Path {
    pub id: NodeId,
    #[params]
    pub params: PathParams,
}

impl Default for Path {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: PathParams::default(),
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

#[derive(Debug, Clone)]
pub(crate) struct PathFrameBinding {
    pub(crate) node_id: NodeId,
    pub(crate) data: Deferred<String>,
    pub(crate) position: Deferred<(f64, f64)>,
    pub(crate) fill_enabled: Deferred<bool>,
    pub(crate) fill_color: Deferred<[u8; 4]>,
    pub(crate) stroke_enabled: Deferred<bool>,
    pub(crate) stroke_color: Deferred<[u8; 4]>,
    pub(crate) stroke_width: Deferred<f64>,
    pub(crate) params_buffer: lumen_gpu::BufferId,
    pub(crate) points_buffer: lumen_gpu::BufferId,
    pub(crate) max_points: usize,
}

impl GpuFrameBinding for PathFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let path_data = self.data.resolve_string(
            self.node_id,
            "data",
            &ctx.expr_context(self.node_id, "data"),
        )?;
        let points = parse_path_points(&path_data, self.max_points);
        let (x, y) = self.position.resolve_vec2(
            self.node_id,
            "position",
            &ctx.expr_context(self.node_id, "position"),
        )?;
        let fill = self.fill_color.resolve_color(
            self.node_id,
            "fill_color",
            &ctx.expr_context(self.node_id, "fill_color"),
        )?;
        let stroke = self.stroke_color.resolve_color(
            self.node_id,
            "stroke_color",
            &ctx.expr_context(self.node_id, "stroke_color"),
        )?;
        let mut flags = 0;
        if self.fill_enabled.resolve_bool(
            self.node_id,
            "fill_enabled",
            &ctx.expr_context(self.node_id, "fill_enabled"),
        )? {
            flags |= 1;
        }
        if self.stroke_enabled.resolve_bool(
            self.node_id,
            "stroke_enabled",
            &ctx.expr_context(self.node_id, "stroke_enabled"),
        )? {
            flags |= 2;
        }

        let params = super::renderer::PathParams {
            fill_color: rgba8_to_f32(fill),
            stroke_color: rgba8_to_f32(stroke),
            position: [x as f32, y as f32],
            stroke_width: self.stroke_width.resolve_float(
                self.node_id,
                "stroke_width",
                &ctx.expr_context(self.node_id, "stroke_width"),
            )? as f32,
            flags,
            point_count: points.len() as u32,
            _pad: [0; 3],
        };
        bound.write_buffer(self.params_buffer, 0, bytemuck::bytes_of(&params));
        if !points.is_empty() {
            bound.write_buffer(self.points_buffer, 0, bytemuck::cast_slice(&points));
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
