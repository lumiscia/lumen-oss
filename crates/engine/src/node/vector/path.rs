use std::borrow::Cow;

use super::drawable::{
    DeferredTransform, DrawableMotion, DrawableTransform, largest_scale, peak_scale,
};
use super::paint::Paint;
use super::renderer::{
    MAX_PATH_POINTS, PathBuffers, PathPoint, PathUniforms, domain_origin, style_flags,
};
use crate::error::RenderError;
use crate::gpu::{
    BoundFrame, CompileContext, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    compiler,
};
use crate::node::motion::Shutter;
use crate::node::{Deferred, DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

const PATH_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("paint/paint.wgsl"),
    include_str!("drawable.wgsl"),
    include_str!("vector_common.wgsl"),
    include_str!("path_renderer.wgsl")
);

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
    /// Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the
    /// usual film look and 360 smears over the whole frame.
    #[meta(min = 0, max = 360, step = 1)]
    pub motion_blur: f64,
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
            motion_blur: 0.0,
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
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "path", name = "Path", category = "vector")]
pub struct Path {
    pub id: NodeId,
    #[params]
    pub params: PathParamsDelegate,
}

impl GpuCompileNode for Path {
    fn compile_gpu(&self, ctx: &mut CompileContext<'_>) -> crate::Result<CompiledOutput> {
        let fixed_points = fixed_points(&self.params.data, &self.params.scale);
        // Opacity only hides what is drawn, so it is left out of the bounds.
        let opaque = PathParamsDelegate {
            opacity: Deferred::value(1.0),
            ..self.params.clone()
        };
        let (output, buffers) = compile_path_source(
            ctx,
            self.id,
            "path",
            fixed_points.as_ref().map_or(MAX_PATH_POINTS, Vec::len),
            compiler::SourceMotion::of(path_is_animated(&self.params), &self.params),
            |expr, band_capacity| {
                path_frame(
                    &opaque,
                    self.id,
                    expr,
                    fixed_points.as_deref(),
                    band_capacity,
                )
            },
        );
        ctx.register_compiled_node(CompiledPath {
            node_id: self.id,
            params: self.params.clone(),
            fixed_points,
            buffers,
        });
        Ok(output)
    }
}

/// Compiles a source rasterized by the path shader, sized to the screen bounds that
/// `frame_at` gives over the frames it may draw. `frame_at` also takes the bands buffer's
/// capacity.
pub(super) fn compile_path_source<'a>(
    ctx: &mut CompileContext<'_>,
    node_id: NodeId,
    label: &str,
    point_capacity: usize,
    motion: compiler::SourceMotion,
    frame_at: impl Fn(&crate::expr::ExpressionContext<'_>, usize) -> crate::Result<PathFrame<'a>>,
) -> (CompiledOutput, PathBuffers) {
    let point_capacity = point_capacity.max(1);
    let band_capacity = super::path_bands::band_capacity(point_capacity);
    let bounds = ctx.source_bounds(motion, |expr| {
        frame_at(expr, band_capacity).map(|frame| Some(frame.uniforms.screen_bounds))
    });
    let domain = ctx.source_domain(bounds);
    let source = ctx.compute_source(
        node_id,
        label,
        "rasterize",
        PATH_SHADER,
        std::mem::size_of::<PathUniforms>() as u64,
        None,
        [
            (
                "points",
                (point_capacity * std::mem::size_of::<PathPoint>()) as u64,
            ),
            ("bands", (band_capacity * std::mem::size_of::<u32>()) as u64),
        ],
        domain,
    );
    let [points, bands] = source.storage;
    (
        CompiledOutput::Raster(source.output),
        PathBuffers {
            params: source.params,
            points,
            bands,
            point_capacity,
            band_capacity,
            origin: domain_origin(domain),
        },
    )
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledPath {
    pub(crate) node_id: NodeId,
    pub(crate) params: PathParamsDelegate,
    /// The flattened data, when nothing can change it per frame.
    pub(crate) fixed_points: Option<Vec<PathPoint>>,
    pub(crate) buffers: PathBuffers,
}

impl GpuCompiledNode for CompiledPath {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let frame = path_frame(
            &self.params,
            self.node_id,
            &ctx.expr_context(),
            self.fixed_points.as_deref(),
            self.buffers.band_capacity,
        )?;
        self.buffers.write(bound, &frame);
        Ok(())
    }
}

/// Evaluates a path node's uniforms, points and bands at the frame of `expr`. `fixed_points`
/// stands in for flattening its data.
fn path_frame<'a>(
    params: &PathParamsDelegate,
    node_id: NodeId,
    expr: &crate::expr::ExpressionContext<'_>,
    fixed_points: Option<&'a [PathPoint]>,
    band_capacity: usize,
) -> crate::Result<PathFrame<'a>> {
    let evaluated = params.eval(&DelegateEvalContext { node_id, expr })?;
    let transform = DrawableTransform {
        position: evaluated.position,
        scale: evaluated.scale,
        rotation: evaluated.rotation,
        anchor: evaluated.anchor,
        opacity: evaluated.opacity,
    };
    let shutter = DeferredTransform {
        position: &params.position,
        scale: &params.scale,
        rotation: &params.rotation,
        anchor: &params.anchor,
    }
    .shutter(evaluated.motion_blur, transform, node_id, expr)?;
    let points = match fixed_points {
        Some(points) => Cow::Borrowed(points),
        // Curves are flattened for the largest scale that the shutter draws them at.
        None => Cow::Owned(
            parse_path_points(
                &evaluated.data,
                flattening_tolerance(peak_scale(evaluated.scale, shutter.as_ref())),
                MAX_PATH_POINTS,
            )
            .map_err(|details| RenderError::NodeEvaluation {
                node_id,
                node_kind: "Path",
                details: format!("`data` {details}"),
            })?,
        ),
    };
    let (x, y) = evaluated.position;
    let (trim_start, trim_end) = normalized_trim_range(evaluated.trim_start, evaluated.trim_end);

    let uniforms = PathUniforms {
        fill_paint: evaluated.fill_paint.to_gpu([255, 255, 255, 255]),
        stroke_paint: evaluated.stroke_paint.to_gpu([0, 0, 0, 255]),
        position: [x as f32, y as f32],
        stroke_width: evaluated.stroke_width.max(0.0) as f32,
        flags: style_flags(
            evaluated.fill_enabled,
            evaluated.stroke_enabled && evaluated.stroke_width > 0.0,
            evaluated.edge_antialias,
        ),
        trim_start,
        trim_end,
        ..bytemuck::Zeroable::zeroed()
    };
    Ok(PathFrame::new(
        uniforms,
        points,
        &transform,
        shutter.as_ref(),
        band_capacity,
    ))
}

/// Whether a path node's canvas bounds may change between frames.
fn path_is_animated(params: &PathParamsDelegate) -> bool {
    params.data.is_expr()
        || params.position.is_expr()
        || params.scale.is_expr()
        || params.rotation.is_expr()
        || params.anchor.is_expr()
        || params.motion_blur.is_expr()
        || params.fill_enabled.is_expr()
        || params.stroke_enabled.is_expr()
        || params.stroke_width.is_expr()
}

/// A frame's uniforms, points and bands for the path shader.
pub(super) struct PathFrame<'a> {
    pub(super) uniforms: PathUniforms,
    points: Cow<'a, [PathPoint]>,
    bands: Vec<u32>,
}

impl<'a> PathFrame<'a> {
    /// Fills in the bounds, transform, culling and motion of `uniforms` and bins `points`.
    pub(super) fn new(
        mut uniforms: PathUniforms,
        points: Cow<'a, [PathPoint]>,
        transform: &DrawableTransform,
        shutter: Option<&Shutter<DrawableTransform>>,
        band_capacity: usize,
    ) -> Self {
        let bounds = point_bounds(&points);
        uniforms.bounds_min = bounds.0;
        uniforms.bounds_size = [
            (bounds.1[0] - bounds.0[0]).max(1.0),
            (bounds.1[1] - bounds.0[1]).max(1.0),
        ];
        uniforms.transform = transform.to_gpu();
        let [x, y] = uniforms.position;
        let min = [x + uniforms.bounds_min[0], y + uniforms.bounds_min[1]];
        let max = [
            min[0] + uniforms.bounds_size[0],
            min[1] + uniforms.bounds_size[1],
        ];
        let motion = DrawableMotion::new(shutter, min, max);
        let bands = super::path_bands::bin_path(
            &mut uniforms,
            &points,
            bounds,
            transform,
            &motion,
            band_capacity,
        );
        uniforms.motion = motion.to_gpu();
        Self {
            uniforms,
            points,
            bands,
        }
    }
}

impl PathBuffers {
    pub(super) fn write(&self, bound: &mut BoundFrame, frame: &PathFrame) {
        let mut uniforms = frame.uniforms;
        uniforms.origin = self.origin;
        bound.write_buffer(self.params, 0, bytemuck::bytes_of(&uniforms));
        if !frame.points.is_empty() {
            bound.write_buffer(self.points, 0, bytemuck::cast_slice(&frame.points));
        }
        if !frame.bands.is_empty() {
            bound.write_buffer(self.bands, 0, bytemuck::cast_slice(&frame.bands));
        }
    }
}

/// Canvas-space flattening tolerance in pixels; curves are flattened untransformed, so
/// the local tolerance shrinks as the path is scaled up.
const PATH_FLATTENING_TOLERANCE: f32 = 0.25;
const PATH_MAX_FLATTENING_ATTEMPTS: usize = 12;
/// f32 loses whole pixels above this, and the limit keeps relative sums and arc arithmetic
/// finite without tracking the current point.
const MAX_PATH_COORDINATE: f64 = 1e7;
// Bits of `PathPoint::flags`; keep in sync with `path_renderer.wgsl`.
pub(super) const POINT_END: u32 = 1;
pub(super) const POINT_CLOSED: u32 = 2;

fn flattening_tolerance(scale: (f64, f64)) -> f32 {
    (f64::from(PATH_FLATTENING_TOLERANCE) / largest_scale(scale)) as f32
}

/// The flattened points of path data whose data and scale are constant. Data that does not
/// flatten has none, so that binding it reports why.
fn fixed_points(data: &Deferred<String>, scale: &Deferred<(f64, f64)>) -> Option<Vec<PathPoint>> {
    let (Deferred::Value(data), Deferred::Value(scale)) = (data, scale) else {
        return None;
    };
    parse_path_points(data, flattening_tolerance(*scale), MAX_PATH_POINTS).ok()
}

/// Flattens path data, coarsening `tolerance` until it fits `max_points`, and measures its
/// contours. Empty data is empty geometry. The error says why data is malformed, has an
/// out-of-range coordinate, or exceeds `max_points` even at the coarsest tolerance.
fn parse_path_points(
    data: &str,
    tolerance: f32,
    max_points: usize,
) -> Result<Vec<PathPoint>, String> {
    let path = build_lyon_path(data)?;

    let mut tolerance = tolerance;
    for _ in 0..PATH_MAX_FLATTENING_ATTEMPTS {
        if let Some(mut points) = flatten_path(&path, tolerance, max_points) {
            normalize_contour_offsets(&mut points);
            return Ok(points);
        }
        tolerance *= 2.0;
    }
    Err(format!("flattens to more than {max_points} points"))
}

/// Builds a single closed contour from polygon vertices.
pub(super) fn closed_contour(vertices: &[[f32; 2]]) -> Vec<PathPoint> {
    let mut points: Vec<_> = vertices
        .iter()
        .map(|&position| PathPoint {
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

fn build_lyon_path(data: &str) -> Result<lyon_path::Path, String> {
    use lyon_path::{
        builder::SvgPathBuilder,
        geom::{ArcFlags, SvgArc},
        math::{Angle, point, vector},
    };
    use svgtypes::PathSegment;

    let mut builder = lyon_path::Path::svg_builder();
    for segment in svgtypes::PathParser::from(data) {
        let segment = segment.map_err(|error| format!("is malformed: {error}"))?;
        match segment {
            PathSegment::MoveTo { abs, x, y } => {
                in_range(&[x, y])?;
                if abs {
                    builder.move_to(point(x as f32, y as f32));
                } else {
                    builder.relative_move_to(vector(x as f32, y as f32));
                }
            }
            PathSegment::LineTo { abs, x, y } => {
                in_range(&[x, y])?;
                if abs {
                    builder.line_to(point(x as f32, y as f32));
                } else {
                    builder.relative_line_to(vector(x as f32, y as f32));
                }
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                in_range(&[x])?;
                if abs {
                    builder.horizontal_line_to(x as f32);
                } else {
                    builder.relative_horizontal_line_to(x as f32);
                }
            }
            PathSegment::VerticalLineTo { abs, y } => {
                in_range(&[y])?;
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
                in_range(&[x1, y1, x2, y2, x, y])?;
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
                in_range(&[x2, y2, x, y])?;
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
                in_range(&[x1, y1, x, y])?;
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
                in_range(&[x, y])?;
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
                in_range(&[rx, ry, x_axis_rotation, x, y])?;
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
                // lyon asserts on chords so short their squared terms underflow.
                if arc.is_straight_line() || (to - from).square_length() < 1e-24 {
                    builder.line_to(to);
                } else {
                    let mut finite = true;
                    arc.for_each_quadratic_bezier(&mut |curve| {
                        finite &= curve.ctrl.x.is_finite()
                            && curve.ctrl.y.is_finite()
                            && curve.to.x.is_finite()
                            && curve.to.y.is_finite();
                        if finite {
                            builder.quadratic_bezier_to(curve.ctrl, curve.to);
                        }
                    });
                    if !finite {
                        return Err("has an arc too extreme to flatten".to_string());
                    }
                    builder.line_to(to);
                }
            }
            PathSegment::ClosePath { .. } => builder.close(),
        }
    }
    Ok(builder.build())
}

fn in_range(values: &[f64]) -> Result<(), String> {
    if values
        .iter()
        .all(|&value| value.abs() <= MAX_PATH_COORDINATE)
    {
        Ok(())
    } else {
        Err(format!("has a coordinate outside +/-{MAX_PATH_COORDINATE}"))
    }
}

fn flatten_path(
    path: &lyon_path::Path,
    tolerance: f32,
    max_points: usize,
) -> Option<Vec<PathPoint>> {
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
    points: &mut Vec<PathPoint>,
    point: lyon_path::math::Point,
    contour_start: u32,
    max_points: usize,
) -> bool {
    if points.len() >= max_points {
        return false;
    }
    points.push(PathPoint {
        position: [point.x, point.y],
        contour_start,
        flags: 0,
        offset: 0.0,
        _pad: 0.0,
    });
    true
}

/// The corners `(min, max)` of the box around `points`, both the origin when there are none.
pub(super) fn point_bounds(points: &[PathPoint]) -> ([f32; 2], [f32; 2]) {
    let Some(first) = points.first() else {
        return ([0.0, 0.0], [0.0, 0.0]);
    };
    points
        .iter()
        .fold((first.position, first.position), |(min, max), point| {
            (
                [min[0].min(point.position[0]), min[1].min(point.position[1])],
                [max[0].max(point.position[0]), max[1].max(point.position[1])],
            )
        })
}

fn point_distance(from: [f32; 2], to: [f32; 2]) -> f32 {
    (to[0] - from[0]).hypot(to[1] - from[1])
}

fn normalized_trim_range(start: f64, end: f64) -> (f32, f32) {
    (start.clamp(0.0, 1.0) as f32, end.clamp(0.0, 1.0) as f32)
}

fn normalize_contour_offsets(points: &mut [PathPoint]) {
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
    fn malformed_or_non_finite_paths_are_errors() {
        let error =
            |data: &str| parse_path_points(data, PATH_FLATTENING_TOLERANCE, 128).unwrap_err();
        for data in ["M 0 0 C nope", "M 1e999 0 L 10 10"] {
            assert!(error(data).contains("malformed"), "{data}");
        }
        for data in [
            "M 0 0 L 1e300 0",
            "M 3e38 0 l 3e38 0",
            "M 0 0 A 1e20 1e20 0 0 1 10 10",
        ] {
            assert!(error(data).contains("outside"), "{data}");
        }
        assert!(
            parse_path_points("", PATH_FLATTENING_TOLERANCE, 128)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn paths_at_the_coordinate_limit_are_finite_or_errors() {
        for data in [
            "M 1e7 1e7 l 1e7 1e7 l 1e7 -1e7 Z",
            "M 0 0 A 1e7 1e7 0 1 1 1e7 1e7",
            "M 0 0 A 1e-30 1e-30 0 0 1 1e7 1e7",
            "M 0 0 A 1 1 0 0 1 1e-25 0",
            "M 1e-30 0 A 5 5 0 0 1 0 0",
        ] {
            let points = parse_path_points(data, PATH_FLATTENING_TOLERANCE, 65536).unwrap();
            assert!(
                points
                    .iter()
                    .all(|p| p.position.iter().all(|c| c.is_finite())),
                "{data}"
            );
        }
        let extreme = "M 0 0 A 1e-3 1e7 45 1 0 -1e7 1e7 a 1e7 1e-3 1e7 0 1 1e7 -1e7";
        let error = parse_path_points(extreme, PATH_FLATTENING_TOLERANCE, 65536).unwrap_err();
        assert!(error.contains("too extreme"), "{error}");
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
            .unwrap_err()
            .contains("more than 4 points")
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
        let fixed = |data: Deferred<String>, scale: Deferred<(f64, f64)>| {
            fixed_points(&data, &scale).map(|points| points.len())
        };
        assert_eq!(
            fixed(Deferred::value(circle(10.0)), Deferred::value((8.0, -4.0))),
            Some(scaled)
        );
        let frame = || crate::expr::Expression::parse("frame").unwrap();
        assert_eq!(
            fixed(Deferred::value(circle(10.0)), Deferred::Expr(frame())),
            None
        );
        assert_eq!(
            fixed(Deferred::Expr(frame()), Deferred::value((1.0, 1.0))),
            None
        );
        assert_eq!(
            fixed(
                Deferred::value("M 0 0 C nope".to_string()),
                Deferred::value((1.0, 1.0))
            ),
            None
        );
    }

    #[test]
    fn normalizes_each_contour_by_its_drawn_length() {
        let points = parse_path_points(
            "M 0 0 L 25 0 L 100 0 M 0 10 L 0 30 Z",
            PATH_FLATTENING_TOLERANCE,
            128,
        )
        .unwrap();

        assert_eq!(points[0].offset, 0.0);
        assert_eq!(points[1].offset, 0.25);
        assert_eq!(points[2].offset, 1.0);
        assert_eq!(points[3].offset, 0.0);
        assert_eq!(points[4].offset, 0.5);
    }

    #[test]
    fn degenerate_contour_offsets_remain_finite() {
        let points = parse_path_points("M 5 5 L 5 5", PATH_FLATTENING_TOLERANCE, 128).unwrap();
        assert!(points.iter().all(|point| point.offset.is_finite()));
    }

    #[test]
    fn trim_range_clamps_without_reordering_or_expanding_empty_ranges() {
        assert_eq!(normalized_trim_range(-1.0, 2.0), (0.0, 1.0));
        assert_eq!(normalized_trim_range(0.75, 0.25), (0.75, 0.25));
        assert_eq!(normalized_trim_range(0.5, 0.5), (0.5, 0.5));
    }
}
