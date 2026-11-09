//! Path data in expressions: sampled by arc length for `path_point` and `path_angle`, and
//! interpolated by `lerp`, `tween` and `keyframes`.

use std::{cell::RefCell, collections::VecDeque, fmt::Write, ops::Range, rc::Rc};

use lyon_path::{
    PathEvent,
    geom::{CubicBezierSegment, LineSegment, QuadraticBezierSegment},
    math::{Point, Vector},
};
use svgtypes::PathSegment;

use crate::node::vector::{path::build_lyon_path, renderer::MAX_PATH_POINTS};

/// Most distance in pixels between a curve and the chords its length is measured along.
const MEASURE_TOLERANCE: f32 = 0.1;
const MAX_MEASURE_ATTEMPTS: usize = 12;
const MAX_CACHED_PATHS: usize = 64;
const MAX_CACHED_BYTES: usize = 8 << 20;

/// A point on a path and the direction it heads there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PathSample {
    pub(super) point: (f64, f64),
    /// Degrees clockwise on screen from +x, unwrapped along the path so it never jumps by a
    /// whole turn.
    pub(super) angle: f64,
}

/// The point `t` of the way along `data` by arc length, with `t` clamped to `0..=1`. Moves
/// between contours add no length, and a closed contour's closing segment does.
pub(super) fn sample_path(data: &str, t: f64) -> Result<PathSample, String> {
    let measure = MEASURES.with_borrow_mut(|cache| cache.get(data))?;
    measure.sample(t)
}

thread_local! {
    static MEASURES: RefCell<MeasureCache> = RefCell::default();
}

/// Measured paths by their data, evicted oldest first past `MAX_CACHED_PATHS` or
/// `MAX_CACHED_BYTES`.
#[derive(Default)]
struct MeasureCache {
    measures: foldhash::HashMap<Rc<str>, Rc<PathMeasure>>,
    order: VecDeque<Rc<str>>,
    bytes: usize,
}

impl MeasureCache {
    fn get(&mut self, data: &str) -> Result<Rc<PathMeasure>, String> {
        if let Some(measure) = self.measures.get(data) {
            return Ok(measure.clone());
        }
        let measure = Rc::new(PathMeasure::new(data)?);
        let bytes = entry_bytes(data, &measure);
        if bytes > MAX_CACHED_BYTES {
            return Ok(measure);
        }
        while self.order.len() >= MAX_CACHED_PATHS || self.bytes + bytes > MAX_CACHED_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.measures.remove(&oldest) {
                self.bytes -= entry_bytes(&oldest, &evicted);
            }
        }
        let key: Rc<str> = data.into();
        self.order.push_back(key.clone());
        self.measures.insert(key, measure.clone());
        self.bytes += bytes;
        Ok(measure)
    }
}

fn entry_bytes(data: &str, measure: &PathMeasure) -> usize {
    data.len() + size_of::<PathMeasure>() + measure.pieces.capacity() * size_of::<Piece>()
}

#[derive(Debug, Clone, Copy)]
enum Curve {
    Line(LineSegment<f32>),
    Quadratic(QuadraticBezierSegment<f32>),
    Cubic(CubicBezierSegment<f32>),
}

impl Curve {
    fn sample(&self, t: f32) -> Point {
        match self {
            Self::Line(line) => line.sample(t),
            Self::Quadratic(curve) => curve.sample(t),
            Self::Cubic(curve) => curve.sample(t),
        }
    }

    fn derivative(&self, t: f32) -> Vector {
        match self {
            Self::Line(line) => line.to_vector(),
            Self::Quadratic(curve) => curve.derivative(t),
            Self::Cubic(curve) => curve.derivative(t),
        }
    }
}

/// One chord of a flattened curve: the part of `curve` over `t`.
#[derive(Debug, Clone)]
struct Piece {
    curve: Curve,
    t: Range<f32>,
    /// Arc length from the start of the path to the end of this piece.
    end: f64,
    /// The unwrapped angle at the start of this piece.
    angle: f64,
}

impl Piece {
    /// The direction of the curve at `t`, or of the chord where the curve has no direction.
    fn direction(&self, t: f32) -> Vector {
        let derivative = self.curve.derivative(t);
        if derivative.square_length() > f32::EPSILON {
            derivative
        } else {
            self.curve.sample(self.t.end) - self.curve.sample(self.t.start)
        }
    }

    /// The angle at `t`, the closest to `self.angle` of the angles a whole turn apart.
    fn angle_at(&self, t: f32) -> f64 {
        unwrap_angle(degrees(self.direction(t)), self.angle)
    }
}

/// A flattened path and the arc length along it.
#[derive(Debug)]
struct PathMeasure {
    /// Pieces of nonzero length, in order along the path.
    pieces: Vec<Piece>,
    /// Where the path starts, if it has a point.
    start: Option<Point>,
}

impl PathMeasure {
    /// Measures `data`, coarsening the flattening until it fits `MAX_PATH_POINTS` pieces, as
    /// the path node does.
    fn new(data: &str) -> Result<Self, String> {
        let path = build_lyon_path(data)?;
        let mut tolerance = MEASURE_TOLERANCE;
        for _ in 0..MAX_MEASURE_ATTEMPTS {
            if let Some(mut measure) = Self::flatten(&path, tolerance) {
                measure.pieces.shrink_to_fit();
                return Ok(measure);
            }
            tolerance *= 2.0;
        }
        Err(format!("flattens to more than {MAX_PATH_POINTS} segments"))
    }

    fn flatten(path: &lyon_path::Path, tolerance: f32) -> Option<Self> {
        let mut measure = Self {
            pieces: Vec::new(),
            start: None,
        };
        let mut length = 0.0;
        let mut angle = None;
        let mut push = |pieces: &mut Vec<Piece>, curve: Curve, chord: &LineSegment<f32>, t| {
            let chord_length = f64::from(chord.length());
            if chord_length == 0.0 || pieces.len() > MAX_PATH_POINTS {
                return;
            }
            length += chord_length;
            let mut piece = Piece {
                curve,
                t,
                end: length,
                angle: 0.0,
            };
            let start = degrees(piece.direction(piece.t.start));
            piece.angle = angle.map_or(start, |previous| unwrap_angle(start, previous));
            angle = Some(piece.angle_at(piece.t.end));
            pieces.push(piece);
        };
        for event in path.iter() {
            let pieces = &mut measure.pieces;
            match event {
                PathEvent::Begin { at } => {
                    measure.start.get_or_insert(at);
                }
                PathEvent::Line { from, to } => {
                    let line = LineSegment { from, to };
                    push(pieces, Curve::Line(line), &line, 0.0..1.0);
                }
                PathEvent::End { last, first, close } => {
                    if close {
                        let line = LineSegment {
                            from: last,
                            to: first,
                        };
                        push(pieces, Curve::Line(line), &line, 0.0..1.0);
                    }
                }
                PathEvent::Quadratic { from, ctrl, to } => {
                    let curve = QuadraticBezierSegment { from, ctrl, to };
                    curve.for_each_flattened_with_t(tolerance, &mut |chord, t| {
                        push(pieces, Curve::Quadratic(curve), chord, t);
                    });
                }
                PathEvent::Cubic {
                    from,
                    ctrl1,
                    ctrl2,
                    to,
                } => {
                    let curve = CubicBezierSegment {
                        from,
                        ctrl1,
                        ctrl2,
                        to,
                    };
                    curve.for_each_flattened_with_t(tolerance, &mut |chord, t| {
                        push(pieces, Curve::Cubic(curve), chord, t);
                    });
                }
            }
        }
        (measure.pieces.len() <= MAX_PATH_POINTS).then_some(measure)
    }

    fn sample(&self, t: f64) -> Result<PathSample, String> {
        let Some(start) = self.start else {
            return Err("has no points".to_string());
        };
        let Some(last) = self.pieces.last() else {
            return Ok(PathSample {
                point: (f64::from(start.x), f64::from(start.y)),
                angle: 0.0,
            });
        };
        let distance = t.clamp(0.0, 1.0) * last.end;
        // The piece that `distance` falls in, or the last at the end of the path.
        let index = self
            .pieces
            .partition_point(|piece| piece.end <= distance)
            .min(self.pieces.len() - 1);
        let piece = &self.pieces[index];
        let piece_start = index.checked_sub(1).map_or(0.0, |i| self.pieces[i].end);
        let along = ((distance - piece_start) / (piece.end - piece_start)).clamp(0.0, 1.0);
        let at = piece.t.start + (piece.t.end - piece.t.start) * along as f32;
        let point = piece.curve.sample(at);
        Ok(PathSample {
            point: (f64::from(point.x), f64::from(point.y)),
            angle: piece.angle_at(at),
        })
    }
}

fn degrees(direction: Vector) -> f64 {
    f64::from(direction.y)
        .atan2(f64::from(direction.x))
        .to_degrees()
}

/// `angle` moved by whole turns to within half a turn of `near`.
fn unwrap_angle(angle: f64, near: f64) -> f64 {
    near + (angle - near + 180.0).rem_euclid(360.0) - 180.0
}

/// Why two path data strings don't interpolate.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum MorphError {
    /// The side `side`, 0 for `from` and 1 for `to`, isn't path data.
    Malformed {
        side: usize,
        details: String,
    },
    /// Command `command`, counting from 1, is described on each side, or `None` where that side
    /// has no more commands.
    Differs {
        command: usize,
        from: Option<String>,
        to: Option<String>,
    },
    NotFinite,
}

impl MorphError {
    /// The error, naming the sides `names`.
    pub(super) fn describe(&self, names: [&str; 2]) -> String {
        match self {
            Self::Malformed { side, details } => format!(
                "strings interpolate only as path data, and {} is malformed path data: {details}",
                names[*side]
            ),
            Self::Differs { command, from, to } => {
                format!(
                    "path data differs at command {command}: {} has {} where {} has {}; only \
                     paths with the same commands in the same order interpolate",
                    names[0],
                    from.as_deref().unwrap_or("no command"),
                    names[1],
                    to.as_deref().unwrap_or("no command"),
                )
            }
            Self::NotFinite => {
                "path data interpolates to a coordinate that isn't finite".to_string()
            }
        }
    }
}

/// Interpolates two path data strings with the same commands in the same order, point by point.
/// The result is path data with each command written out, its numbers rounded to the `f32` that
/// the path node draws with. Numbers equal on both sides aren't mixed, so they don't jitter from
/// frame to frame.
pub(super) fn morph(from: &str, to: &str, t: f64) -> Result<String, MorphError> {
    let commands = |data: &str, side| {
        svgtypes::PathParser::from(data)
            .map(|segment| segment.map(Command::new))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| MorphError::Malformed {
                side,
                details: error.to_string(),
            })
    };
    let (from, to) = (commands(from, 0)?, commands(to, 1)?);
    let mut data = String::new();
    for index in 0..from.len().max(to.len()) {
        let (from, to) = match (from.get(index), to.get(index)) {
            (Some(from), Some(to))
                if from.letter == to.letter && from.arc_flags == to.arc_flags =>
            {
                (from, to)
            }
            (from, to) => {
                return Err(MorphError::Differs {
                    command: index + 1,
                    from: from.map(Command::describe),
                    to: to.map(Command::describe),
                });
            }
        };
        if index > 0 {
            data.push(' ');
        }
        data.push(from.letter);
        for (value, (&from_value, &to_value)) in from.values().iter().zip(to.values()).enumerate() {
            if let (Some((large_arc, sweep)), 3) = (from.arc_flags, value) {
                let _ = write!(data, " {} {}", u8::from(large_arc), u8::from(sweep));
            }
            let mixed = if from_value == to_value {
                from_value
            } else {
                from_value * (1.0 - t) + to_value * t
            };
            let mixed = mixed as f32;
            if !mixed.is_finite() {
                return Err(MorphError::NotFinite);
            }
            let _ = write!(data, " {mixed}");
        }
    }
    Ok(data)
}

/// A path command and its numbers, with an arc's flags apart from them.
struct Command {
    /// The command's letter, lowercase when relative.
    letter: char,
    values: [f64; 7],
    count: usize,
    /// An arc's large-arc and sweep flags, which go between its rotation and end point.
    arc_flags: Option<(bool, bool)>,
}

impl Command {
    fn new(segment: PathSegment) -> Self {
        let command = |abs: bool, letter: char, numbers: &[f64]| {
            let mut values = [0.0; 7];
            values[..numbers.len()].copy_from_slice(numbers);
            Self {
                letter: if abs {
                    letter
                } else {
                    letter.to_ascii_lowercase()
                },
                values,
                count: numbers.len(),
                arc_flags: None,
            }
        };
        match segment {
            PathSegment::MoveTo { abs, x, y } => command(abs, 'M', &[x, y]),
            PathSegment::LineTo { abs, x, y } => command(abs, 'L', &[x, y]),
            PathSegment::HorizontalLineTo { abs, x } => command(abs, 'H', &[x]),
            PathSegment::VerticalLineTo { abs, y } => command(abs, 'V', &[y]),
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => command(abs, 'C', &[x1, y1, x2, y2, x, y]),
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => command(abs, 'S', &[x2, y2, x, y]),
            PathSegment::Quadratic { abs, x1, y1, x, y } => command(abs, 'Q', &[x1, y1, x, y]),
            PathSegment::SmoothQuadratic { abs, x, y } => command(abs, 'T', &[x, y]),
            PathSegment::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => Self {
                arc_flags: Some((large_arc, sweep)),
                ..command(abs, 'A', &[rx, ry, x_axis_rotation, x, y])
            },
            // `z` and `Z` both close the contour.
            PathSegment::ClosePath { .. } => command(true, 'Z', &[]),
        }
    }

    fn values(&self) -> &[f64] {
        &self.values[..self.count]
    }

    fn describe(&self) -> String {
        match self.arc_flags {
            Some((large_arc, sweep)) => format!(
                "`{}` with large-arc flag {} and sweep flag {}",
                self.letter,
                u8::from(large_arc),
                u8::from(sweep)
            ),
            None => format!("`{}`", self.letter),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{Expression, ExpressionContext, ExpressionValue};

    fn evaluate(source: &str) -> Result<ExpressionValue, String> {
        Expression::parse(source)
            .map_err(|error| error.to_string())?
            .evaluate(&ExpressionContext::default())
            .map_err(|error| error.to_string())
    }

    fn point(data: &str, t: f64) -> (f64, f64) {
        sample_path(data, t).unwrap().point
    }

    fn angle(data: &str, t: f64) -> f64 {
        sample_path(data, t).unwrap().angle
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} is not within {tolerance} of {expected}"
        );
    }

    #[test]
    fn points_are_spaced_evenly_by_arc_length() {
        // Two legs of lengths 100 and 300.
        let legs = "M 0 0 L 100 0 L 100 300";
        assert_eq!(point(legs, 0.25), (100.0, 0.0));
        assert_eq!(point(legs, 0.5), (100.0, 100.0));
        assert_eq!(point(legs, 0.125), (50.0, 0.0));

        // A cubic along a straight line, whose parameter races and crawls. Points are as even
        // as the measuring tolerance.
        let line = "M 0 0 C 190 0 195 0 200 0";
        for i in 0..=20 {
            let t = f64::from(i) / 20.0;
            assert_close(point(line, t).0, 200.0 * t, f64::from(MEASURE_TOLERANCE));
        }

        // On a half circle about (100, 100), the point turns through equal angles.
        let arc = "M 0 100 A 100 100 0 0 1 200 100";
        for i in 0..20 {
            let (x, y) = point(arc, f64::from(i) / 20.0);
            let turn = (100.0 - y).atan2(100.0 - x).to_degrees();
            assert_close(turn, 9.0 * f64::from(i), 0.05);
        }
    }

    #[test]
    fn endpoints_are_the_ends_of_the_path_and_t_is_clamped() {
        let curve = "M 10 20 C 100 0 100 100 200 100";
        assert_eq!(point(curve, 0.0), (10.0, 20.0));
        assert_eq!(point(curve, 1.0), (200.0, 100.0));
        assert_eq!(point(curve, -3.0), point(curve, 0.0));
        assert_eq!(point(curve, f64::INFINITY), point(curve, 1.0));

        // A closed contour ends where it starts, and a move adds no length.
        assert_eq!(point("M 0 0 H 100 V 100 Z", 1.0), (0.0, 0.0));
        assert_eq!(point("M 0 0 H 100 M 0 50 H 100", 0.5), (0.0, 50.0));
        assert_eq!(point("M 5 7", 0.5), (5.0, 7.0));
    }

    #[test]
    fn short_segments_far_from_the_origin_still_count() {
        assert_eq!(point("M 1000000 0 L 1000001 0", 1.0), (1000001.0, 0.0));
        assert_eq!(angle("M 1000000 0 L 1000000 1", 0.5), 90.0);
        let corner = "M 1000000 0 L 1000001 0 L 1000001 1";
        assert_eq!(point(corner, 0.75), (1000001.0, 0.5));
        assert_eq!(angle(corner, 0.75), 90.0);
        assert_eq!(angle("M 1000000 0 L 1000001 0 M 0 0 L 0 1", 0.75), 90.0);
        let after_curve = "M 1000000 0 Q 1000000.5 0 1000001 0 L 1000001 1";
        assert_eq!(angle(after_curve, 0.75), 90.0);
        // An arc resets the control a smooth quadratic reflects.
        for smooth in ["T 300 100", "t 100 0"] {
            let data = format!("M 0 100 A 100 100 0 0 1 200 100 {smooth}");
            assert_close(angle(&data, 1.0), 0.0, 1e-9);
            assert_close(point(&data, 0.9).1, 100.0, 1e-9);
        }
    }

    #[test]
    fn angles_follow_the_tangent_in_clockwise_degrees() {
        assert_eq!(angle("M 0 0 L 0 100", 0.5), 90.0);
        // The upper half of a circle of radius 100 drawn clockwise on screen from (0, 100):
        // the tangent turns from straight up to straight down.
        let arc = "M 0 100 A 100 100 0 0 1 200 100";
        assert_close(angle(arc, 0.0), -90.0, 0.01);
        assert_close(angle(arc, 0.5), 0.0, 0.01);
        assert_close(angle(arc, 1.0), 90.0, 0.01);
        // Turning continues past a half turn instead of wrapping.
        let circle = "M 0 100 A 100 100 0 0 1 200 100 A 100 100 0 0 1 0 100";
        assert_close(angle(circle, 1.0), 270.0, 0.01);
    }

    #[test]
    fn builtins_return_a_vec2_and_degrees() {
        assert_eq!(
            evaluate("path_point('M 0 0 L 100 0', 0.25)").unwrap(),
            ExpressionValue::Vec2((25.0, 0.0))
        );
        assert_eq!(
            evaluate("path_angle('M 0 0 L -100 0', 0.5)").unwrap(),
            ExpressionValue::Number(180.0)
        );
        for (source, expected) in [
            ("path_point('M 0 0 Q', 0)", "`path_data` is malformed"),
            ("path_point('', 0)", "`path_data` has no points"),
            ("path_angle(1, 0)", "`path_data` must be a string"),
            (
                "path_angle('M 0 0 L 1 1', pow(-1, 0.5))",
                "`t` must not be NaN",
            ),
        ] {
            let error = evaluate(source).unwrap_err();
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    fn string(source: &str, frame: f64) -> Result<String, String> {
        let ctx = ExpressionContext {
            frame,
            ..ExpressionContext::default()
        };
        match Expression::parse(source)
            .map_err(|error| error.to_string())?
            .evaluate(&ctx)
            .map_err(|error| error.to_string())?
        {
            ExpressionValue::String(data) => Ok(data),
            other => panic!("{source} is not a string: {other:?}"),
        }
    }

    const TRIANGLE: &str = "M 0 0 L 100 0 L 50 80 Z";
    const FLIPPED: &str = "M 0 40 L 100 40 L 50 0 Z";

    #[test]
    fn lerp_morphs_path_data_point_by_point() {
        let lerp = |t: f64| string(&format!("lerp('{TRIANGLE}', '{FLIPPED}', {t})"), 0.0);
        assert_eq!(lerp(0.0).unwrap(), TRIANGLE);
        assert_eq!(lerp(1.0).unwrap(), FLIPPED);
        assert_eq!(lerp(0.5).unwrap(), "M 0 20 L 100 20 L 50 40 Z");
        assert_eq!(
            string(&format!("tween('{TRIANGLE}', '{FLIPPED}', 0, 10)"), 5.0).unwrap(),
            "M 0 20 L 100 20 L 50 40 Z"
        );

        // Every command keeps its letter and case, implicit commands are written out, and an
        // arc's flags stay put.
        let from = "M 0 0 10 0 h 5 v 5 C 0 0 1 1 2 2 s 3 3 4 4 Q 1 1 2 2 t 4 4 A 5 5 0 1 0 9 9 z";
        let to = "M 0 2 10 2 h 7 v 7 C 2 2 3 3 4 4 s 5 5 6 6 Q 3 3 4 4 t 6 6 A 7 7 90 1 0 11 11 Z";
        let mid = string(&format!("lerp('{from}', '{to}', 0.5)"), 0.0).unwrap();
        assert_eq!(
            mid,
            "M 0 1 L 10 1 h 6 v 6 C 1 1 2 2 3 3 s 4 4 5 5 Q 2 2 3 3 t 5 5 A 6 6 45 1 0 10 10 Z"
        );
        build_lyon_path(&mid).unwrap();
    }

    #[test]
    fn morphed_numbers_are_as_precise_as_the_path_node_draws() {
        // f64 mixing gives 0.15000000000000002 and 0.30000000000000004, which draw as the f32
        // nearest 0.15 and 0.3. Numbers the paths share aren't mixed.
        assert_eq!(
            morph(
                "M 0.1 7.123456789 L 0.1 0",
                "M 0.2 7.123456789 L 0.5 0",
                0.5
            )
            .unwrap(),
            "M 0.15 7.123457 L 0.3 0"
        );
        for step in 0..=10 {
            let data = morph("M 0 0.1 L 3 3", "M 30 0.1 L 3 9", f64::from(step) / 10.0).unwrap();
            assert!(
                data.starts_with(&format!("M {} 0.1 L 3 ", step * 3)),
                "{data}"
            );
        }
        assert_eq!(
            morph("M 0 0", "M 1 0", f64::NAN),
            Err(MorphError::NotFinite)
        );
        assert_eq!(morph("M 0 0", "M 1 0", 1e40), Err(MorphError::NotFinite));
    }

    #[test]
    fn keyframes_morph_path_data() {
        let track =
            format!("keyframes(frame, key(0, '{TRIANGLE}', 'in_quad'), key(30, '{FLIPPED}'))");
        assert_eq!(string(&track, -5.0).unwrap(), TRIANGLE);
        assert_eq!(string(&track, 15.0).unwrap(), "M 0 10 L 100 10 L 50 60 Z");
        assert_eq!(string(&track, 30.0).unwrap(), FLIPPED);
        let track =
            format!("keyframes(frame, key(0, '{TRIANGLE}'), key(30, '{FLIPPED}', 'in_out_cubic'))");
        assert_eq!(string(&track, 15.0).unwrap(), "M 0 20 L 100 20 L 50 40 Z");
    }

    #[test]
    fn keys_that_dont_interpolate_may_differ_in_structure() {
        const CURVE: &str = "M 0 0 Q 5 10 10 0";
        // A hold switches shapes at the next key.
        for track in [
            format!("keyframes(frame, key(0, '{TRIANGLE}', 'hold'), key(10, '{CURVE}'))"),
            format!("keyframes(frame, 'hold', key(0, '{TRIANGLE}'), key(10, '{CURVE}'))"),
            format!(
                "keyframes(frame, key(0, '{TRIANGLE}', 'hold'), key(10, if(frame > 0, '{CURVE}', '')))"
            ),
        ] {
            assert_eq!(string(&track, 5.0).unwrap(), TRIANGLE, "{track}");
            assert_eq!(string(&track, 10.0).unwrap(), CURVE, "{track}");
        }
        // An easing that is 0 partway still interpolates, so the structures must match.
        let track = format!(
            "keyframes(frame, cubic_bezier(vec2(0, 0), vec2(1, -1 / 3)), key(0, '{TRIANGLE}'), key(10, '{CURVE}'))"
        );
        assert!(
            string(&track, 5.0)
                .unwrap_err()
                .contains("path data differs")
        );
        // Keys at the same time jump, whatever their units.
        let track = format!("keyframes(frame, key(0, '{TRIANGLE}'), key(0s, '{CURVE}'))");
        assert_eq!(string(&track, 0.0).unwrap(), CURVE);
        let track = format!(
            "keyframes(frame, key(0, '{TRIANGLE}'), key(10, '{TRIANGLE}'), key(10, '{CURVE}'))"
        );
        assert_eq!(string(&track, 10.0).unwrap(), CURVE);
        // Only neighbouring keys interpolate, so a key the parser can't read separates the
        // literal paths around it.
        let track = format!(
            "keyframes(frame, key(0, 'M 0 0 L 10 0'), key(10, if(frame < 10, 'M 0 0 L 20 0', '{CURVE}')), key(20, 'M 0 0 Q 10 20 20 0'))"
        );
        assert_eq!(string(&track, 5.0).unwrap(), "M 0 0 L 15 0");
        assert_eq!(string(&track, 15.0).unwrap(), "M 0 0 Q 7.5 15 15 0");
    }

    #[test]
    fn paths_of_different_structure_name_the_first_command_that_differs() {
        let differs = "path data differs at command";
        for (source, expected) in [
            (
                "lerp('M 0 0 L 1 1 L 2 2', 'M 0 0 L 1 1 C 1 1 2 2 3 3', 0.5)".to_string(),
                format!("`lerp`: {differs} 3: `from` has `L` where `to` has `C`"),
            ),
            (
                "lerp('M 0 0 L 1 1', 'M 0 0 l 1 1', 0.5)".to_string(),
                format!("{differs} 2: `from` has `L` where `to` has `l`"),
            ),
            (
                "tween('M 0 0 L 1 1', 'M 0 0', 0, 10)".to_string(),
                format!("{differs} 2: `from` has `L` where `to` has no command"),
            ),
            (
                "lerp('M 0 0 A 1 1 0 0 1 2 2', 'M 0 0 A 1 1 0 1 1 2 2', 0.5)".to_string(),
                format!(
                    "{differs} 2: `from` has `A` with large-arc flag 0 and sweep flag 1 where \
                     `to` has `A` with large-arc flag 1 and sweep flag 1"
                ),
            ),
            (
                format!("keyframes(frame, key(0s, '{TRIANGLE}'), key(1s, 'M 0 0 Z'))"),
                format!("{differs} 2: the key at 0s has `L` where the key at 1s has `Z`"),
            ),
            // Paths the parser can't see are checked when they interpolate.
            (
                format!(
                    "keyframes(frame, key(0, '{TRIANGLE}'), key(10, if(frame > 0, 'M 0 0 Z', '')))"
                ),
                format!("{differs} 2: the key at 0 has `L` where the key at 10 has `Z`"),
            ),
            (
                "lerp('M 0 0', lowercase('hello'), 0.5)".to_string(),
                "`lerp`: strings interpolate only as path data, and `to` is malformed path data"
                    .to_string(),
            ),
            (
                "lerp('hello', 'M 0 0', 0.5)".to_string(),
                "and `from` is malformed path data".to_string(),
            ),
            (
                "lerp('M 0 0', 1, 0.5)".to_string(),
                "numbers, vec2 values, colours or path data, got string and number".to_string(),
            ),
        ] {
            let error = string(&source, 5.0).unwrap_err();
            assert!(error.contains(&expected), "{source}: {error}");
        }
    }

    #[test]
    fn the_cache_evicts_the_oldest_paths_past_its_bounds() {
        let mut cache = MeasureCache::default();
        let data = |index: usize| format!("M 0 0 L {index} 1");
        for index in 0..MAX_CACHED_PATHS + 5 {
            cache.get(&data(index)).unwrap();
        }
        assert_eq!(cache.measures.len(), MAX_CACHED_PATHS);
        assert!(!cache.measures.contains_key(data(4).as_str()));
        assert!(cache.measures.contains_key(data(5).as_str()));
        let first = cache.get(&data(5)).unwrap();
        assert!(Rc::ptr_eq(&first, &cache.get(&data(5)).unwrap()));
        assert_eq!(
            cache.bytes,
            cache
                .measures
                .iter()
                .map(|(data, measure)| entry_bytes(data, measure))
                .sum::<usize>()
        );
    }

    #[test]
    fn measured_pieces_hold_no_spare_capacity() {
        let data = format!("M 0 0{}", " l 1 1".repeat(8193));
        let measure = PathMeasure::new(&data).unwrap();
        assert_eq!(measure.pieces.capacity(), measure.pieces.len());
    }
}
