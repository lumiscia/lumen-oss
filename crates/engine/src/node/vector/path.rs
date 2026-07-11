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
    /// Normalized start of the visible stroke range for each contour.
    #[meta(name = "Trim start", min = 0, max = 1, step = 0.01)]
    pub trim_start: f64,
    /// Normalized end of the visible stroke range for each contour.
    #[meta(name = "Trim end", min = 0, max = 1, step = 0.01)]
    pub trim_end: f64,
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
            trim_start: 0.0,
            trim_end: 1.0,
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
        let mut points = parse_path_points(&evaluated.data, self.max_points);
        normalize_contour_offsets(&mut points);
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
            trim_start: evaluated.trim_start.clamp(0.0, 1.0) as f32,
            trim_end: evaluated.trim_end.clamp(0.0, 1.0) as f32,
            _pad: 0,
        };
        bound.write_buffer(self.params_buffer, 0, bytemuck::bytes_of(&params));
        if !points.is_empty() {
            bound.write_buffer(self.points_buffer, 0, bytemuck::cast_slice(&points));
        }
        Ok(())
    }
}

const PATH_FLATTENING_TOLERANCE: f32 = 0.25;
const PATH_MAX_FLATTENING_ATTEMPTS: usize = 12;
const PATH_END: u32 = 1;
const PATH_CLOSED: u32 = 2;

fn parse_path_points(data: &str, max_points: usize) -> Vec<super::renderer::PathPoint> {
    let Some(path) = build_lyon_path(data) else {
        return Vec::new();
    };
    if max_points == 0 {
        return Vec::new();
    }

    let mut tolerance = PATH_FLATTENING_TOLERANCE;
    for _ in 0..PATH_MAX_FLATTENING_ATTEMPTS {
        if let Some(points) = flatten_path(&path, tolerance, max_points) {
            return points;
        }
        tolerance *= 2.0;
    }

    flatten_path_capped(&path, tolerance, max_points)
}

fn build_lyon_path(data: &str) -> Option<lyon_path::Path> {
    use lyon_path::{
        builder::SvgPathBuilder,
        geom::{ArcFlags, SvgArc},
        math::{Angle, point, vector},
    };
    use svgtypes::PathSegment;

    let mut builder = lyon_path::Path::svg_builder();
    for segment in svgtypes::PathParser::from(data) {
        let segment = segment.ok()?;
        match segment {
            PathSegment::MoveTo { abs, x, y } => {
                finite(&[x, y])?;
                if abs {
                    builder.move_to(point(x as f32, y as f32));
                } else {
                    builder.relative_move_to(vector(x as f32, y as f32));
                }
            }
            PathSegment::LineTo { abs, x, y } => {
                finite(&[x, y])?;
                if abs {
                    builder.line_to(point(x as f32, y as f32));
                } else {
                    builder.relative_line_to(vector(x as f32, y as f32));
                }
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                finite(&[x])?;
                if abs {
                    builder.horizontal_line_to(x as f32);
                } else {
                    builder.relative_horizontal_line_to(x as f32);
                }
            }
            PathSegment::VerticalLineTo { abs, y } => {
                finite(&[y])?;
                if abs {
                    builder.vertical_line_to(y as f32);
                } else {
                    builder.relative_vertical_line_to(y as f32);
                }
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
                finite(&[x1, y1, x2, y2, x, y])?;
                if abs {
                    builder.cubic_bezier_to(
                        point(x1 as f32, y1 as f32),
                        point(x2 as f32, y2 as f32),
                        point(x as f32, y as f32),
                    );
                } else {
                    builder.relative_cubic_bezier_to(
                        vector(x1 as f32, y1 as f32),
                        vector(x2 as f32, y2 as f32),
                        vector(x as f32, y as f32),
                    );
                }
            }
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                finite(&[x2, y2, x, y])?;
                if abs {
                    builder.smooth_cubic_bezier_to(
                        point(x2 as f32, y2 as f32),
                        point(x as f32, y as f32),
                    );
                } else {
                    builder.smooth_relative_cubic_bezier_to(
                        vector(x2 as f32, y2 as f32),
                        vector(x as f32, y as f32),
                    );
                }
            }
            PathSegment::Quadratic { abs, x1, y1, x, y } => {
                finite(&[x1, y1, x, y])?;
                if abs {
                    builder.quadratic_bezier_to(
                        point(x1 as f32, y1 as f32),
                        point(x as f32, y as f32),
                    );
                } else {
                    builder.relative_quadratic_bezier_to(
                        vector(x1 as f32, y1 as f32),
                        vector(x as f32, y as f32),
                    );
                }
            }
            PathSegment::SmoothQuadratic { abs, x, y } => {
                finite(&[x, y])?;
                if abs {
                    builder.smooth_quadratic_bezier_to(point(x as f32, y as f32));
                } else {
                    builder.smooth_relative_quadratic_bezier_to(vector(x as f32, y as f32));
                }
            }
            PathSegment::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                finite(&[rx, ry, x_axis_rotation, x, y])?;
                let from = builder.current_position();
                let to = if abs {
                    point(x as f32, y as f32)
                } else {
                    from + vector(x as f32, y as f32)
                };
                let arc = SvgArc {
                    from,
                    to,
                    radii: vector(rx.abs() as f32, ry.abs() as f32),
                    x_rotation: Angle::degrees(x_axis_rotation as f32),
                    flags: ArcFlags { large_arc, sweep },
                };
                if arc.is_straight_line() {
                    builder.line_to(to);
                } else {
                    arc.for_each_quadratic_bezier(&mut |curve| {
                        builder.quadratic_bezier_to(curve.ctrl, curve.to);
                    });
                    builder.line_to(to);
                }
            }
            PathSegment::ClosePath { .. } => builder.close(),
        }
    }
    Some(builder.build())
}

fn finite(values: &[f64]) -> Option<()> {
    values.iter().all(|value| value.is_finite()).then_some(())
}

fn flatten_path(
    path: &lyon_path::Path,
    tolerance: f32,
    max_points: usize,
) -> Option<Vec<super::renderer::PathPoint>> {
    collect_flattened_path(path, tolerance, max_points, false)
}

fn flatten_path_capped(
    path: &lyon_path::Path,
    tolerance: f32,
    max_points: usize,
) -> Vec<super::renderer::PathPoint> {
    collect_flattened_path(path, tolerance, max_points, true).unwrap_or_default()
}

fn collect_flattened_path(
    path: &lyon_path::Path,
    tolerance: f32,
    max_points: usize,
    truncate: bool,
) -> Option<Vec<super::renderer::PathPoint>> {
    use lyon_path::{PathEvent, iterator::PathIterator};

    let mut points = Vec::new();
    let mut contour_start = 0_u32;
    for event in path.iter().flattened(tolerance) {
        match event {
            PathEvent::Begin { at } => {
                contour_start = points.len() as u32;
                if !push_path_point(&mut points, at, contour_start, max_points) {
                    return truncated(points, truncate);
                }
            }
            PathEvent::Line { to, .. } => {
                if points
                    .last()
                    .is_none_or(|last| last.position != [to.x, to.y])
                {
                    if !push_path_point(&mut points, to, contour_start, max_points) {
                        return truncated(points, truncate);
                    }
                }
            }
            PathEvent::End { close, .. } => {
                if let Some(last) = points.last_mut()
                    && last.contour_start == contour_start
                {
                    last.flags |= PATH_END;
                    if close {
                        last.flags |= PATH_CLOSED;
                    }
                }
            }
            PathEvent::Quadratic { .. } | PathEvent::Cubic { .. } => unreachable!(),
        }
    }
    Some(points)
}

fn push_path_point(
    points: &mut Vec<super::renderer::PathPoint>,
    point: lyon_path::math::Point,
    contour_start: u32,
    max_points: usize,
) -> bool {
    if points.len() >= max_points {
        return false;
    }
    points.push(super::renderer::PathPoint {
        position: [point.x, point.y],
        contour_start,
        flags: 0,
        offset: 0.0,
        _pad: 0.0,
    });
    true
}

fn truncated(
    mut points: Vec<super::renderer::PathPoint>,
    truncate: bool,
) -> Option<Vec<super::renderer::PathPoint>> {
    if !truncate {
        return None;
    }
    if let Some(last) = points.last_mut() {
        last.flags |= PATH_END;
        last.flags &= !PATH_CLOSED;
    }
    Some(points)
}

fn bounds_min(points: &[super::renderer::PathPoint]) -> [f32; 2] {
    let Some(first) = points.first() else {
        return [0.0, 0.0];
    };
    points.iter().skip(1).fold(first.position, |acc, point| {
        [acc[0].min(point.position[0]), acc[1].min(point.position[1])]
    })
}

fn point_distance(from: [f32; 2], to: [f32; 2]) -> f32 {
    (to[0] - from[0]).hypot(to[1] - from[1])
}

fn normalize_contour_offsets(points: &mut [super::renderer::PathPoint]) {
    let mut start = 0;
    while start < points.len() {
        let end = points[start..]
            .iter()
            .position(|point| point.flags & PATH_END != 0)
            .map_or(points.len() - 1, |offset| start + offset);
        let closed = points[end].flags & PATH_CLOSED != 0;
        let closing_length = if closed && end > start {
            point_distance(points[end].position, points[start].position)
        } else {
            0.0
        };
        let total_length = points[start..=end]
            .windows(2)
            .map(|pair| point_distance(pair[0].position, pair[1].position))
            .sum::<f32>()
            + closing_length;
        if total_length > f32::EPSILON {
            let mut length = 0.0;
            for index in start + 1..=end {
                length += point_distance(points[index - 1].position, points[index].position);
                points[index].offset = length / total_length;
            }
        }
        start = end + 1;
    }
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
        let points = parse_path_points("M 10 20 h 10 v 10 l -5 5 z", 128);
        assert_eq!(
            points
                .iter()
                .map(|point| point.position)
                .collect::<Vec<_>>(),
            vec![[10.0, 20.0], [20.0, 20.0], [20.0, 30.0], [15.0, 35.0]]
        );
        assert_eq!(points.last().unwrap().flags, PATH_END | PATH_CLOSED);
    }

    #[test]
    fn flattens_cubic_and_smooth_curves() {
        let points = positions("M 0 0 C 0 100 100 100 100 0 S 200 -100 200 0");
        assert!(points.len() > 8);
        assert_eq!(points[0], [0.0, 0.0]);
        assert_eq!(*points.last().unwrap(), [200.0, 0.0]);
        assert!(points.iter().any(|point| point[1] > 70.0));
        assert!(points.iter().any(|point| point[1] < -70.0));
    }

    #[test]
    fn flattens_quadratic_and_smooth_curves() {
        let points = positions("M 0 0 Q 50 100 100 0 T 200 0");
        assert!(points.len() > 8);
        assert_eq!(points[0], [0.0, 0.0]);
        assert_eq!(*points.last().unwrap(), [200.0, 0.0]);
        assert!(points.iter().any(|point| point[1] > 45.0));
        assert!(points.iter().any(|point| point[1] < -45.0));
    }

    #[test]
    fn flattens_elliptical_arcs() {
        let points = positions("M 0 0 A 50 25 30 0 1 100 0");
        assert!(points.len() > 3);
        assert_eq!(points[0], [0.0, 0.0]);
        assert_eq!(*points.last().unwrap(), [100.0, 0.0]);
        assert!(
            points[1..points.len() - 1]
                .iter()
                .any(|point| point[1].abs() > 1.0)
        );
    }

    #[test]
    fn preserves_subpath_boundaries_and_closure() {
        let points = parse_path_points("M 0 0 L 10 0 M 100 0 L 110 0 Z", 128);
        assert_eq!(points.len(), 4);
        assert_eq!(points[1].flags, PATH_END);
        assert_eq!(points[1].contour_start, 0);
        assert_eq!(points[2].contour_start, 2);
        assert_eq!(points[3].contour_start, 2);
        assert_eq!(points[3].flags, PATH_END | PATH_CLOSED);
    }

    #[test]
    fn malformed_or_non_finite_paths_are_empty() {
        assert!(parse_path_points("M 0 0 C nope", 128).is_empty());
        assert!(parse_path_points("M 1e999 0 L 10 10", 128).is_empty());
    }

    #[test]
    fn adaptive_tolerance_controls_point_count() {
        let path = build_lyon_path("M 0 0 C 0 1000 1000 1000 1000 0").unwrap();
        let fine = flatten_path(&path, 0.1, 1024).unwrap();
        let coarse = flatten_path(&path, 10.0, 1024).unwrap();
        assert!(fine.len() > coarse.len());
        assert_eq!(fine.first().unwrap().position, [0.0, 0.0]);
        assert_eq!(fine.last().unwrap().position, [1000.0, 0.0]);
        assert_eq!(coarse.last().unwrap().position, [1000.0, 0.0]);
    }

    #[test]
    fn smooth_controls_reset_after_non_curve_commands() {
        let cubic = positions("M 0 0 C 0 100 100 100 100 0 L 200 0 S 300 100 300 0");
        assert!(
            cubic
                .iter()
                .filter(|point| point[0] >= 200.0)
                .all(|point| point[1] >= -f32::EPSILON)
        );

        let quadratic = positions("M 0 0 Q 50 100 100 0 L 200 0 T 300 0");
        assert!(
            quadratic
                .iter()
                .filter(|point| point[0] >= 200.0)
                .all(|point| point[1].abs() <= f32::EPSILON)
        );
    }

    #[test]
    fn caps_flattened_output_to_gpu_buffer_capacity() {
        let points = parse_path_points("M 0 0 C 0 1000 1000 1000 1000 0", 5);
        assert!(points.len() <= 5);
        assert_eq!(points.first().unwrap().position, [0.0, 0.0]);
        assert_eq!(points.last().unwrap().position, [1000.0, 0.0]);
        assert_ne!(points.last().unwrap().flags & PATH_END, 0);
    }

    #[test]
    fn normalizes_each_contour_by_its_drawn_length() {
        let mut points = parse_path_points("M 0 0 L 25 0 L 100 0 M 0 10 L 0 30 Z", 128);
        normalize_contour_offsets(&mut points);

        assert_eq!(points[0].offset, 0.0);
        assert_eq!(points[1].offset, 0.25);
        assert_eq!(points[2].offset, 1.0);
        assert_eq!(points[3].offset, 0.0);
        assert_eq!(points[4].offset, 0.5);
    }

    #[test]
    fn degenerate_contour_offsets_remain_finite() {
        let mut points = parse_path_points("M 5 5 L 5 5", 128);
        normalize_contour_offsets(&mut points);
        assert!(points.iter().all(|point| point.offset.is_finite()));
    }
}
