use super::drawable::{DrawableTransform, largest_scale};
use super::paint::Paint;
use super::renderer::{MAX_PATH_POINTS, PathBuffers, PathPoint, PathUniforms, style_flags};
use crate::error::RenderError;
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};
use crate::node::{Deferred, NodeId, NodeParamEvalContext, NodeParams, PortRef};

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
    /// Scale multiplier per axis, about the anchor.
    #[meta()]
    pub scale: (f64, f64),
    /// Clockwise rotation in degrees, about the anchor.
    #[meta(step = 1)]
    pub rotation: f64,
    /// Pivot for scale and rotation, in pixels relative to `position`.
    #[meta()]
    pub anchor: (f64, f64),
    /// Opacity multiplier.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
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
    /// Stroke width in untransformed pixels. Under non-uniform scale the stroke stretches
    /// with the path, so it is wider across the more scaled axis.
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
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            opacity: 1.0,
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
    pub(crate) buffers: PathBuffers,
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
        let point_capacity = self.buffers.point_capacity;
        let mut points = parse_path_points(
            &evaluated.data,
            flattening_tolerance(evaluated.scale),
            point_capacity,
        )
        .ok_or_else(|| RenderError::NodeEvaluation {
            frame: ctx.frame(),
            node_id: self.node_id,
            node_kind: "Path",
            details: format!("path data flattens to more than {point_capacity} points"),
        })?;
        normalize_contour_offsets(&mut points);
        let (x, y) = evaluated.position;
        let (trim_start, trim_end) =
            normalized_trim_range(evaluated.trim_start, evaluated.trim_end);

        let params = PathUniforms {
            fill_paint: evaluated.fill_paint.to_gpu([255, 255, 255, 255]),
            stroke_paint: evaluated.stroke_paint.to_gpu([0, 0, 0, 255]),
            position: [x as f32, y as f32],
            bounds_min: bounds_min(&points),
            bounds_size: bounds_size(&points),
            stroke_width: evaluated.stroke_width as f32,
            flags: style_flags(
                evaluated.fill_enabled,
                evaluated.stroke_enabled,
                evaluated.edge_antialias,
            ),
            point_count: points.len() as u32,
            trim_start,
            trim_end,
            ..bytemuck::Zeroable::zeroed()
        };
        let transform = DrawableTransform {
            position: evaluated.position,
            scale: evaluated.scale,
            rotation: evaluated.rotation,
            anchor: evaluated.anchor,
            opacity: evaluated.opacity,
        };
        self.buffers.write(bound, params, &points, &transform);
        Ok(())
    }
}

impl PathBuffers {
    /// Writes a frame's uniforms, points and bands for the path shader.
    pub(super) fn write(
        &self,
        bound: &mut BoundFrame,
        mut uniforms: PathUniforms,
        points: &[PathPoint],
        transform: &DrawableTransform,
    ) {
        uniforms.transform = transform.to_gpu();
        let bands =
            super::path_bands::bin_path(&mut uniforms, points, transform, self.band_capacity);
        bound.write_buffer(self.params, 0, bytemuck::bytes_of(&uniforms));
        if !points.is_empty() {
            bound.write_buffer(self.points, 0, bytemuck::cast_slice(points));
        }
        if !bands.is_empty() {
            bound.write_buffer(self.bands, 0, bytemuck::cast_slice(&bands));
        }
    }
}

/// Canvas-space flattening tolerance in pixels; curves are flattened untransformed, so
/// the local tolerance shrinks as the path is scaled up.
const PATH_FLATTENING_TOLERANCE: f32 = 0.25;
const PATH_MAX_FLATTENING_ATTEMPTS: usize = 12;
// Bits of `PathPoint::flags`; keep in sync with `path_renderer.wgsl`.
pub(super) const POINT_END: u32 = 1;
pub(super) const POINT_CLOSED: u32 = 2;

fn flattening_tolerance(scale: (f64, f64)) -> f32 {
    (f64::from(PATH_FLATTENING_TOLERANCE) / largest_scale(scale)) as f32
}

/// Points buffer capacity for path data: the flattened point count when the data and
/// scale are constant, or `MAX_PATH_POINTS` when either may change per frame.
pub(super) fn point_capacity(data: &Deferred<String>, scale: &Deferred<(f64, f64)>) -> usize {
    match (data, scale) {
        (Deferred::Value(data), Deferred::Value(scale)) => {
            parse_path_points(data, flattening_tolerance(*scale), MAX_PATH_POINTS)
                .map_or(MAX_PATH_POINTS, |points| points.len().max(1))
        }
        _ => MAX_PATH_POINTS,
    }
}

/// Flattens path data, coarsening `tolerance` until it fits `max_points`. Malformed
/// data is empty; `None` means it exceeds `max_points` even at the coarsest tolerance.
fn parse_path_points(
    data: &str,
    tolerance: f32,
    max_points: usize,
) -> Option<Vec<super::renderer::PathPoint>> {
    let Some(path) = build_lyon_path(data) else {
        return Some(Vec::new());
    };

    let mut tolerance = tolerance;
    for _ in 0..PATH_MAX_FLATTENING_ATTEMPTS {
        if let Some(points) = flatten_path(&path, tolerance, max_points) {
            return Some(points);
        }
        tolerance *= 2.0;
    }
    None
}

/// Builds a single closed contour from polygon vertices.
pub(super) fn closed_contour(vertices: &[[f32; 2]]) -> Vec<super::renderer::PathPoint> {
    let mut points: Vec<_> = vertices
        .iter()
        .map(|&position| super::renderer::PathPoint {
            position,
            contour_start: 0,
            flags: 0,
            offset: 0.0,
            _pad: 0.0,
        })
        .collect();
    if let Some(last) = points.last_mut() {
        last.flags = POINT_END | POINT_CLOSED;
    }
    normalize_contour_offsets(&mut points);
    points
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
    use lyon_path::{PathEvent, iterator::PathIterator};

    let mut points = Vec::new();
    let mut contour_start = 0_u32;
    for event in path.iter().flattened(tolerance) {
        match event {
            PathEvent::Begin { at } => {
                contour_start = points.len() as u32;
                if !push_path_point(&mut points, at, contour_start, max_points) {
                    return None;
                }
            }
            PathEvent::Line { to, .. } => {
                if points
                    .last()
                    .is_none_or(|last| last.position != [to.x, to.y])
                    && !push_path_point(&mut points, to, contour_start, max_points)
                {
                    return None;
                }
            }
            PathEvent::End { close, .. } => {
                if let Some(last) = points.last_mut()
                    && last.contour_start == contour_start
                {
                    last.flags |= POINT_END;
                    if close {
                        last.flags |= POINT_CLOSED;
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

pub(super) fn bounds_min(points: &[super::renderer::PathPoint]) -> [f32; 2] {
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

fn normalized_trim_range(start: f64, end: f64) -> (f32, f32) {
    (start.clamp(0.0, 1.0) as f32, end.clamp(0.0, 1.0) as f32)
}

fn normalize_contour_offsets(points: &mut [super::renderer::PathPoint]) {
    let mut start = 0;
    while start < points.len() {
        let end = points[start..]
            .iter()
            .position(|point| point.flags & POINT_END != 0)
            .map_or(points.len() - 1, |offset| start + offset);
        let closed = points[end].flags & POINT_CLOSED != 0;
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

pub(super) fn bounds_size(points: &[super::renderer::PathPoint]) -> [f32; 2] {
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
        parse_path_points(data, PATH_FLATTENING_TOLERANCE, 128)
            .unwrap()
            .into_iter()
            .map(|point| point.position)
            .collect()
    }

    #[test]
    fn parses_absolute_relative_and_close_commands() {
        let points =
            parse_path_points("M 10 20 h 10 v 10 l -5 5 z", PATH_FLATTENING_TOLERANCE, 128)
                .unwrap();
        assert_eq!(
            points
                .iter()
                .map(|point| point.position)
                .collect::<Vec<_>>(),
            vec![[10.0, 20.0], [20.0, 20.0], [20.0, 30.0], [15.0, 35.0]]
        );
        assert_eq!(points.last().unwrap().flags, POINT_END | POINT_CLOSED);
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
        let points = parse_path_points(
            "M 0 0 L 10 0 M 100 0 L 110 0 Z",
            PATH_FLATTENING_TOLERANCE,
            128,
        )
        .unwrap();
        assert_eq!(points.len(), 4);
        assert_eq!(points[1].flags, POINT_END);
        assert_eq!(points[1].contour_start, 0);
        assert_eq!(points[2].contour_start, 2);
        assert_eq!(points[3].contour_start, 2);
        assert_eq!(points[3].flags, POINT_END | POINT_CLOSED);
    }

    #[test]
    fn malformed_or_non_finite_paths_are_empty() {
        assert!(
            parse_path_points("M 0 0 C nope", PATH_FLATTENING_TOLERANCE, 128)
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_path_points("M 1e999 0 L 10 10", PATH_FLATTENING_TOLERANCE, 128)
                .unwrap()
                .is_empty()
        );
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
    fn coarsens_curves_to_fit_capacity() {
        let points = parse_path_points(
            "M 0 0 C 0 1000 1000 1000 1000 0",
            PATH_FLATTENING_TOLERANCE,
            5,
        )
        .unwrap();
        assert!(points.len() <= 5);
        assert_eq!(points.first().unwrap().position, [0.0, 0.0]);
        assert_eq!(points.last().unwrap().position, [1000.0, 0.0]);
        assert_ne!(points.last().unwrap().flags & POINT_END, 0);
    }

    #[test]
    fn rejects_paths_with_more_corners_than_capacity() {
        assert!(
            parse_path_points(
                "M 0 0 L 10 0 L 10 10 L 0 10 L 5 5 Z",
                PATH_FLATTENING_TOLERANCE,
                4
            )
            .is_none()
        );
        assert_eq!(
            point_capacity(
                &Deferred::value("M 0 0 L 10 0 L 10 10 Z".to_string()),
                &Deferred::value((1.0, 1.0))
            ),
            3
        );
        assert_eq!(
            point_capacity(
                &Deferred::value(String::new()),
                &Deferred::value((1.0, 1.0))
            ),
            1
        );
    }

    #[test]
    fn scaled_curves_flatten_as_finely_as_curves_drawn_at_that_size() {
        let circle = |radius: f64| {
            format!(
                "M {radius} 0 A {radius} {radius} 0 1 1 -{radius} 0 A {radius} {radius} 0 1 1 {radius} 0 Z"
            )
        };
        let count = |data: &str, scale: f64| {
            parse_path_points(
                data,
                flattening_tolerance((scale, -scale * 0.5)),
                MAX_PATH_POINTS,
            )
            .unwrap()
            .len()
        };
        let scaled = count(&circle(10.0), 8.0);
        let direct = count(&circle(80.0), 1.0);
        assert!(
            scaled.abs_diff(direct) <= 2,
            "{scaled} points against {direct}"
        );
        assert_eq!(
            point_capacity(
                &Deferred::value(circle(10.0)),
                &Deferred::value((8.0, -4.0))
            ),
            scaled
        );
        assert_eq!(
            point_capacity(
                &Deferred::value(circle(10.0)),
                &Deferred::Expr(crate::expr::Expression::parse("frame").unwrap())
            ),
            MAX_PATH_POINTS
        );
    }

    #[test]
    fn normalizes_each_contour_by_its_drawn_length() {
        let mut points = parse_path_points(
            "M 0 0 L 25 0 L 100 0 M 0 10 L 0 30 Z",
            PATH_FLATTENING_TOLERANCE,
            128,
        )
        .unwrap();
        normalize_contour_offsets(&mut points);

        assert_eq!(points[0].offset, 0.0);
        assert_eq!(points[1].offset, 0.25);
        assert_eq!(points[2].offset, 1.0);
        assert_eq!(points[3].offset, 0.0);
        assert_eq!(points[4].offset, 0.5);
    }

    #[test]
    fn degenerate_contour_offsets_remain_finite() {
        let mut points = parse_path_points("M 5 5 L 5 5", PATH_FLATTENING_TOLERANCE, 128).unwrap();
        normalize_contour_offsets(&mut points);
        assert!(points.iter().all(|point| point.offset.is_finite()));
    }

    #[test]
    fn trim_range_clamps_without_reordering_or_expanding_empty_ranges() {
        assert_eq!(normalized_trim_range(-1.0, 2.0), (0.0, 1.0));
        assert_eq!(normalized_trim_range(0.75, 0.25), (0.75, 0.25));
        assert_eq!(normalized_trim_range(0.5, 0.5), (0.5, 0.5));
    }
}
