use bytemuck::{Pod, Zeroable};

use super::super::dash::GpuDash;
use super::super::drawable::largest_scale;

/// Most points a path may flatten to.
pub(crate) const MAX_PATH_POINTS: usize = 16384;

// Bits of a drawable's `flags`; keep in sync with `vector_common.wgsl`.
pub(super) const STYLE_FILL_ENABLED: u32 = 1;
pub(super) const STYLE_STROKE_ENABLED: u32 = 2;
const STYLE_EDGE_ANTIALIAS: u32 = 4;

pub(super) fn style_flags(fill: bool, stroke: bool, edge_antialias: bool) -> u32 {
    let mut flags = 0;
    if fill {
        flags |= STYLE_FILL_ENABLED;
    }
    if stroke {
        flags |= STYLE_STROKE_ENABLED;
    }
    if edge_antialias {
        flags |= STYLE_EDGE_ANTIALIAS;
    }
    flags
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "PathPoint", source = super::build::DRAWING_SHADER)]
pub(crate) struct PathPoint {
    pub(crate) position: [f32; 2],
    pub(crate) contour_start: u32,
    pub(crate) flags: u32,
    /// Fraction of its contour's length at which the point lies.
    pub(crate) offset: f32,
    /// Distance into the dash pattern at which the point lies, within one period. A contour's
    /// first point holds it where the contour ends instead, after its closing segment.
    pub(crate) dash_phase: f32,
}

/// Canvas-space flattening tolerance in pixels; curves are flattened untransformed, so
/// the local tolerance shrinks as the path is scaled up.
const PATH_FLATTENING_TOLERANCE: f32 = 0.25;
const PATH_MAX_FLATTENING_ATTEMPTS: usize = 12;
/// f32 loses whole pixels above this, and the limit keeps relative sums and arc arithmetic
/// finite without tracking the current point. Fill offsets and feathers are clamped to it.
pub(super) const MAX_PATH_COORDINATE: f64 = 1e7;
// Bits of `PathPoint::flags`; keep in sync with `path_geometry.wgsl`.
pub(super) const POINT_END: u32 = 1;
pub(super) const POINT_CLOSED: u32 = 2;
/// The segment from the point is no edge of the fill, though it counts toward its crossings.
pub(super) const POINT_FILL_HIDDEN: u32 = 4;

pub(super) fn flattening_tolerance(scale: (f64, f64)) -> f32 {
    (f64::from(PATH_FLATTENING_TOLERANCE) / largest_scale(scale)) as f32
}

/// Flattens path data, coarsening `tolerance` until it fits `max_points`, and measures its
/// contours. Empty data is empty geometry. The error says why data is malformed, has an
/// out-of-range coordinate, or exceeds `max_points` even at the coarsest tolerance.
pub(super) fn parse_path_points(
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

pub(crate) fn build_lyon_path(data: &str) -> Result<lyon_path::Path, String> {
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
                    // Each curve is drawn once the next arrives, so the last can end exactly at
                    // `to` rather than where rounding put it.
                    let mut pending = None;
                    arc.for_each_quadratic_bezier(&mut |curve| {
                        finite &= curve.ctrl.x.is_finite()
                            && curve.ctrl.y.is_finite()
                            && curve.to.x.is_finite()
                            && curve.to.y.is_finite();
                        if let Some((ctrl, end)) = pending.replace((curve.ctrl, curve.to))
                            && finite
                        {
                            builder.quadratic_bezier_to(ctrl, end);
                        }
                    });
                    if !finite {
                        return Err("has an arc too extreme to flatten".to_string());
                    }
                    if let Some((ctrl, _)) = pending {
                        builder.quadratic_bezier_to(ctrl, to);
                    }
                    // A zero-length line, so a `T` after the arc doesn't reflect its last control.
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

/// The points of closed `contours`, measured as [`parse_path_points`] measures a path's.
pub(super) fn contour_points(contours: &[Vec<[f32; 2]>]) -> Vec<PathPoint> {
    let mut points = Vec::with_capacity(contours.iter().map(Vec::len).sum());
    for contour in contours.iter().filter(|contour| !contour.is_empty()) {
        let contour_start = points.len() as u32;
        points.extend(contour.iter().map(|&position| PathPoint {
            position,
            contour_start,
            flags: 0,
            offset: 0.0,
            dash_phase: 0.0,
        }));
        if let Some(last) = points.last_mut() {
            last.flags |= POINT_END | POINT_CLOSED;
        }
    }
    normalize_contour_offsets(&mut points);
    points
}

/// Index of the point that the segment from point `index` runs to.
fn segment_end(points: &[PathPoint], index: usize) -> usize {
    if points[index].flags & POINT_END != 0 {
        points[index].contour_start as usize
    } else {
        index + 1
    }
}

/// The horizontal or vertical line that segment `a`-`b` lies along, as the axis it runs along
/// and the bits of its other coordinate.
fn axis_line(a: [f32; 2], b: [f32; 2]) -> Option<(usize, u32)> {
    if a[1] == b[1] && a[0] != b[0] {
        Some((0, (a[1] + 0.0).to_bits()))
    } else if a[0] == b[0] && a[1] != b[1] {
        Some((1, (a[0] + 0.0).to_bits()))
    } else {
        None
    }
}

/// Flags the pieces of the outline that the even-odd rule cancels as no edge of the fill, so
/// that its antialiasing, offset and feather measure only its real boundary. Pieces cancel
/// where an even number of them coincide: identical segments, and the overlaps of horizontal
/// or vertical ones on the same line, which are split where the others end first. Segments
/// that only close an open contour for the fill are not split, as splitting them would stroke
/// them. Splitting is skipped if it would take the path past `max_points`.
pub(super) fn hide_cancelled_edges(points: &mut Vec<PathPoint>, max_points: usize) {
    use std::collections::HashMap;

    let closes_open = |flags: u32| flags & (POINT_END | POINT_CLOSED) == POINT_END;
    let mut stops: HashMap<(usize, u32), Vec<f32>> = HashMap::new();
    for index in 0..points.len() {
        let [a, b] = [index, segment_end(points, index)].map(|i| points[i].position);
        if let Some(line) = axis_line(a, b) {
            stops
                .entry(line)
                .or_default()
                .extend([a[line.0] + 0.0, b[line.0] + 0.0]);
        }
    }
    // A line with one segment has its two ends as stops, which split nothing.
    stops.retain(|_, stops| stops.len() > 2);
    for stops in stops.values_mut() {
        stops.sort_by(f32::total_cmp);
        stops.dedup();
    }
    // The stops strictly inside the segment from point `index`, if it may be split.
    let inner_stops = |points: &[PathPoint], index: usize| -> Option<(usize, &[f32])> {
        let [a, b] = [index, segment_end(points, index)].map(|i| points[i].position);
        let line = axis_line(a, b).filter(|_| !closes_open(points[index].flags))?;
        let stops = stops.get(&line)?;
        let (low, high) = (a[line.0].min(b[line.0]), a[line.0].max(b[line.0]));
        let first = stops.partition_point(|&stop| stop <= low);
        let end = stops.partition_point(|&stop| stop < high);
        Some((line.0, &stops[first..end.max(first)]))
    };
    let added: usize = (0..points.len())
        .filter_map(|index| inner_stops(points, index))
        .map(|(_, stops)| stops.len())
        .sum();
    if added > 0 && points.len() + added <= max_points {
        let mut split = Vec::with_capacity(points.len() + added);
        let mut contour_start = 0;
        for (index, point) in points.iter().enumerate() {
            if point.contour_start as usize == index {
                contour_start = split.len() as u32;
            }
            let end_flags = point.flags & (POINT_END | POINT_CLOSED);
            split.push(PathPoint {
                contour_start,
                flags: point.flags & !end_flags,
                ..*point
            });
            if let Some((axis, inner)) = inner_stops(points, index) {
                let forward =
                    point.position[axis] < points[segment_end(points, index)].position[axis];
                let mut place = |&stop: &f32| {
                    let mut position = point.position;
                    position[axis] = stop;
                    split.push(PathPoint {
                        position,
                        contour_start,
                        flags: 0,
                        offset: 0.0,
                        dash_phase: 0.0,
                    });
                };
                if forward {
                    inner.iter().for_each(&mut place);
                } else {
                    inner.iter().rev().for_each(&mut place);
                }
            }
            if let Some(last) = split.last_mut() {
                last.flags |= end_flags;
            }
        }
        *points = split;
        normalize_contour_offsets(points);
    }

    let key = |[x, y]: [f32; 2]| [(x + 0.0).to_bits(), (y + 0.0).to_bits()];
    let pieces: Vec<_> = (0..points.len())
        .map(|index| {
            let [a, b] = [index, segment_end(points, index)].map(|i| key(points[i].position));
            (a.min(b), a.max(b))
        })
        .collect();
    let mut counts: HashMap<_, u32> = HashMap::new();
    for &piece in &pieces {
        *counts.entry(piece).or_default() += 1;
    }
    for (point, piece) in points.iter_mut().zip(&pieces) {
        if piece.0 != piece.1 && counts[piece] % 2 == 0 {
            point.flags |= POINT_FILL_HIDDEN;
        }
    }
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
        dash_phase: 0.0,
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

pub(super) fn normalized_trim_range(start: f64, end: f64) -> (f32, f32) {
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

/// Sets each point's place in `dash`, measuring along its contour in f64 so that it stays
/// precise however long the contour is. A contour's first point takes the place where the
/// contour ends, after its closing segment.
pub(super) fn set_dash_phases(points: &mut [PathPoint], dash: &GpuDash) {
    let mut distance = 0.0;
    for index in 0..points.len() {
        let start = points[index].contour_start as usize;
        if start == index {
            distance = 0.0;
        } else {
            distance += precise_distance(points[index - 1].position, points[index].position);
            points[index].dash_phase = dash.phase(distance);
        }
        if points[index].flags & POINT_END != 0 {
            let closing = precise_distance(points[index].position, points[start].position);
            points[start].dash_phase = dash.phase(distance + closing);
        }
    }
}

fn precise_distance(from: [f32; 2], to: [f32; 2]) -> f64 {
    (f64::from(to[0]) - f64::from(from[0])).hypot(f64::from(to[1]) - f64::from(from[1]))
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
    fn splits_shared_edges_and_hides_the_pieces_that_cancel() {
        // A square beside part of another's right edge, from 30 to 50.
        let mut points = parse_path_points(
            "M 20 20 H 60 V 60 H 20 Z M 60 30 H 80 V 50 H 60 Z",
            PATH_FLATTENING_TOLERANCE,
            128,
        )
        .unwrap();
        hide_cancelled_edges(&mut points, MAX_PATH_POINTS);
        let hidden: Vec<_> = (0..points.len())
            .filter(|&index| points[index].flags & POINT_FILL_HIDDEN != 0)
            .map(|index| {
                [
                    points[index].position,
                    points[segment_end(&points, index)].position,
                ]
            })
            .collect();
        assert_eq!(
            hidden,
            [[[60.0, 30.0], [60.0, 50.0]], [[60.0, 50.0], [60.0, 30.0]]],
            "{points:?}"
        );
        assert_eq!(points.len(), 10);
    }

    #[test]
    fn splitting_past_the_point_limit_is_skipped_before_it_is_built() {
        // Nested segments on one line, each split at every other's ends: 2n² points in all.
        let data: String = (0..4000)
            .map(|i| format!("M {i} 0 H {} Z ", 8000 - i))
            .collect();
        let mut points = parse_path_points(&data, PATH_FLATTENING_TOLERANCE, 65536).unwrap();
        let count = points.len();
        hide_cancelled_edges(&mut points, MAX_PATH_POINTS);
        assert_eq!(points.len(), count);
    }

    #[test]
    fn trim_range_clamps_without_reordering_or_expanding_empty_ranges() {
        assert_eq!(normalized_trim_range(-1.0, 2.0), (0.0, 1.0));
        assert_eq!(normalized_trim_range(0.75, 0.25), (0.75, 0.25));
        assert_eq!(normalized_trim_range(0.5, 0.5), (0.5, 0.5));
    }
}
