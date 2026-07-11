use super::paint::Paint;
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};
use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

/// Produces a rasterized vector path source.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct PathParams {
    /// SVG-style path data.
    #[meta(
        name = "Path data",
        format = "path_data",
        multiline,
        recommended_rows = 5
    )]
    pub data: String,
    /// Path origin in pixels.
    #[meta()]
    pub position: (f64, f64),
    /// Enables fill rendering.
    #[meta()]
    pub fill_enabled: bool,
    /// Fill paint. Accepts a solid color or gradient.
    #[meta()]
    pub fill_paint: Paint,
    /// Enables stroke rendering.
    #[meta()]
    pub stroke_enabled: bool,
    /// Stroke paint. Accepts a solid color or gradient.
    #[meta()]
    pub stroke_paint: Paint,
    /// Stroke width in pixels.
    #[meta(min = 0, step = 0.5)]
    pub stroke_width: f64,
    /// Enables analytic distance-based edge antialiasing.
    #[meta()]
    pub edge_antialias: bool,
}

impl Default for PathParams {
    fn default() -> Self {
        Self {
            data: "M 0 0 L 100 0 L 100 100 L 0 100 Z".to_string(),
            position: (0.0, 0.0),
            fill_enabled: true,
            fill_paint: Paint::solid([255, 255, 255, 255]),
            stroke_enabled: false,
            stroke_paint: Paint::solid([0, 0, 0, 255]),
            stroke_width: 1.0,
            edge_antialias: true,
        }
    }
}

/// Produces a rasterized vector path source.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "path", name = "Path", category = "vector")]
pub struct Path {
    pub id: NodeId,
    #[params]
    pub params: PathParamsDelegate,
}

impl Default for Path {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: PathParamsDelegate::default(),
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
pub(crate) struct CompiledPath {
    pub(crate) node_id: NodeId,
    pub(crate) params: PathParamsDelegate,
    pub(crate) params_buffer: lumen_gpu::BufferId,
    pub(crate) points_buffer: lumen_gpu::BufferId,
    pub(crate) max_points: usize,
}

impl GpuCompiledNode for CompiledPath {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let points = parse_path_points(&evaluated.data, self.max_points);
        let (x, y) = evaluated.position;
        let mut flags = 0;
        if evaluated.fill_enabled {
            flags |= 1;
        }
        if evaluated.stroke_enabled {
            flags |= 2;
        }
        if evaluated.edge_antialias {
            flags |= 4;
        }

        let params = super::renderer::PathParams {
            fill_paint: evaluated.fill_paint.to_gpu([255, 255, 255, 255]),
            stroke_paint: evaluated.stroke_paint.to_gpu([0, 0, 0, 255]),
            position: [x as f32, y as f32],
            bounds_min: bounds_min(&points),
            bounds_size: bounds_size(&points),
            stroke_width: evaluated.stroke_width as f32,
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

fn parse_path_points(data: &str, max_points: usize) -> Vec<super::renderer::PathPoint> {
    use svgtypes::PathSegment;

    const CURVE_STEPS: usize = 16;
    let mut points = Vec::new();
    let mut current = [0.0_f32; 2];
    let mut subpath_start = current;
    let mut cubic_control = None;
    let mut quadratic_control = None;

    let absolute = |abs: bool, x: f64, y: f64, origin: [f32; 2]| {
        let point = [x as f32, y as f32];
        if abs {
            point
        } else {
            [origin[0] + point[0], origin[1] + point[1]]
        }
    };
    let mut push = |point: [f32; 2]| {
        if points.len() < max_points
            && points
                .last()
                .is_none_or(|last: &super::renderer::PathPoint| last.position != point)
        {
            points.push(super::renderer::PathPoint { position: point });
        }
    };

    for segment in svgtypes::PathParser::from(data).flatten() {
        match segment {
            PathSegment::MoveTo { abs, x, y } => {
                current = absolute(abs, x, y, current);
                subpath_start = current;
                push(current);
            }
            PathSegment::LineTo { abs, x, y } => {
                current = absolute(abs, x, y, current);
                push(current);
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                current[0] = if abs { x as f32 } else { current[0] + x as f32 };
                push(current);
            }
            PathSegment::VerticalLineTo { abs, y } => {
                current[1] = if abs { y as f32 } else { current[1] + y as f32 };
                push(current);
            }
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let start = current;
                let control1 = absolute(abs, x1, y1, start);
                let control2 = absolute(abs, x2, y2, start);
                let end = absolute(abs, x, y, start);
                flatten_cubic(&mut push, start, control1, control2, end, CURVE_STEPS);
                current = end;
                cubic_control = Some(control2);
            }
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                let start = current;
                let control1 = cubic_control.map_or(start, |point| reflect(point, start));
                let control2 = absolute(abs, x2, y2, start);
                let end = absolute(abs, x, y, start);
                flatten_cubic(&mut push, start, control1, control2, end, CURVE_STEPS);
                current = end;
                cubic_control = Some(control2);
            }
            PathSegment::Quadratic { abs, x1, y1, x, y } => {
                let start = current;
                let control = absolute(abs, x1, y1, start);
                let end = absolute(abs, x, y, start);
                flatten_quadratic(&mut push, start, control, end, CURVE_STEPS);
                current = end;
                quadratic_control = Some(control);
            }
            PathSegment::SmoothQuadratic { abs, x, y } => {
                let start = current;
                let control = quadratic_control.map_or(start, |point| reflect(point, start));
                let end = absolute(abs, x, y, start);
                flatten_quadratic(&mut push, start, control, end, CURVE_STEPS);
                current = end;
                quadratic_control = Some(control);
            }
            PathSegment::EllipticalArc { abs, x, y, .. } => {
                current = absolute(abs, x, y, current);
                push(current);
            }
            PathSegment::ClosePath { .. } => {
                current = subpath_start;
                push(current);
            }
        }
        if !matches!(
            segment,
            PathSegment::CurveTo { .. } | PathSegment::SmoothCurveTo { .. }
        ) {
            cubic_control = None;
        }
        if !matches!(
            segment,
            PathSegment::Quadratic { .. } | PathSegment::SmoothQuadratic { .. }
        ) {
            quadratic_control = None;
        }
    }
    points
}

fn reflect(point: [f32; 2], around: [f32; 2]) -> [f32; 2] {
    [2.0 * around[0] - point[0], 2.0 * around[1] - point[1]]
}

fn flatten_cubic(
    push: &mut impl FnMut([f32; 2]),
    start: [f32; 2],
    control1: [f32; 2],
    control2: [f32; 2],
    end: [f32; 2],
    steps: usize,
) {
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        let inverse = 1.0 - t;
        push([
            inverse.powi(3) * start[0]
                + 3.0 * inverse.powi(2) * t * control1[0]
                + 3.0 * inverse * t.powi(2) * control2[0]
                + t.powi(3) * end[0],
            inverse.powi(3) * start[1]
                + 3.0 * inverse.powi(2) * t * control1[1]
                + 3.0 * inverse * t.powi(2) * control2[1]
                + t.powi(3) * end[1],
        ]);
    }
}

fn flatten_quadratic(
    push: &mut impl FnMut([f32; 2]),
    start: [f32; 2],
    control: [f32; 2],
    end: [f32; 2],
    steps: usize,
) {
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        let inverse = 1.0 - t;
        push([
            inverse.powi(2) * start[0] + 2.0 * inverse * t * control[0] + t.powi(2) * end[0],
            inverse.powi(2) * start[1] + 2.0 * inverse * t * control[1] + t.powi(2) * end[1],
        ]);
    }
}

fn bounds_min(points: &[super::renderer::PathPoint]) -> [f32; 2] {
    let Some(first) = points.first() else {
        return [0.0, 0.0];
    };
    points.iter().skip(1).fold(first.position, |acc, point| {
        [acc[0].min(point.position[0]), acc[1].min(point.position[1])]
    })
}

fn bounds_size(points: &[super::renderer::PathPoint]) -> [f32; 2] {
    let min = bounds_min(points);
    let max = points.iter().fold(min, |acc, point| {
        [acc[0].max(point.position[0]), acc[1].max(point.position[1])]
    });
    [(max[0] - min[0]).max(1.0), (max[1] - min[1]).max(1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(data: &str) -> Vec<[f32; 2]> {
        parse_path_points(data, 128)
            .into_iter()
            .map(|point| point.position)
            .collect()
    }

    #[test]
    fn parses_absolute_relative_and_close_commands() {
        assert_eq!(
            positions("M 10 20 h 10 v 10 l -5 5 z"),
            vec![
                [10.0, 20.0],
                [20.0, 20.0],
                [20.0, 30.0],
                [15.0, 35.0],
                [10.0, 20.0]
            ]
        );
    }

    #[test]
    fn flattens_cubic_and_smooth_curves() {
        let points = positions("M 0 0 C 0 100 100 100 100 0 S 200 -100 200 0");
        assert_eq!(points.len(), 33);
        assert_eq!(points[0], [0.0, 0.0]);
        assert_eq!(points[16], [100.0, 0.0]);
        assert_eq!(points[32], [200.0, 0.0]);
        assert!(points[8][1] > 70.0);
        assert!(points[24][1] < -70.0);
    }

    #[test]
    fn flattens_quadratic_and_smooth_curves() {
        let points = positions("M 0 0 Q 50 100 100 0 T 200 0");
        assert_eq!(points.len(), 33);
        assert_eq!(points[16], [100.0, 0.0]);
        assert_eq!(points[32], [200.0, 0.0]);
        assert_eq!(points[8], [50.0, 50.0]);
        assert_eq!(points[24], [150.0, -50.0]);
    }

    #[test]
    fn caps_flattened_output_to_gpu_buffer_capacity() {
        assert_eq!(parse_path_points("M 0 0 C 0 100 100 100 100 0", 5).len(), 5);
    }
}
