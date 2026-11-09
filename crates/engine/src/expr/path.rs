//! Sampling path data by arc length for `path_point` and `path_angle`.

use std::{cell::RefCell, collections::VecDeque, ops::Range, rc::Rc};

use lyon_path::{
    PathEvent,
    geom::{CubicBezierSegment, LineSegment, QuadraticBezierSegment},
    math::{Point, Vector},
};

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
    /// Whether the piece is too short for its direction to mean anything, so its angle stays
    /// `angle` throughout.
    sliver: bool,
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
        if self.sliver {
            return self.angle;
        }
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
        // `build_lyon_path` ends each arc's quadratics with a line to the arc's exact end point,
        // which rounding leaves a few ulps long and pointing anywhere. Such a line keeps the
        // angle the path had before it.
        let mut after_quadratic = false;
        let mut push = |pieces: &mut Vec<Piece>,
                        curve: Curve,
                        chord: &LineSegment<f32>,
                        t,
                        may_be_sliver: bool| {
            let chord_length = f64::from(chord.length());
            if chord_length == 0.0 || pieces.len() > MAX_PATH_POINTS {
                return;
            }
            length += chord_length;
            let scale = chord.from.x.abs().max(chord.from.y.abs()).max(1.0);
            let sliver = may_be_sliver
                && angle.is_some()
                && chord_length <= f64::from(scale * f32::EPSILON * 16.0);
            let mut piece = Piece {
                curve,
                t,
                end: length,
                angle: angle.unwrap_or(0.0),
                sliver,
            };
            if !sliver {
                let start = degrees(piece.direction(piece.t.start));
                piece.angle = angle.map_or(start, |previous| unwrap_angle(start, previous));
            }
            angle = Some(piece.angle_at(piece.t.end));
            pieces.push(piece);
        };
        for event in path.iter() {
            let pieces = &mut measure.pieces;
            let follows_quadratic = std::mem::replace(
                &mut after_quadratic,
                matches!(event, PathEvent::Quadratic { .. }),
            );
            match event {
                PathEvent::Begin { at } => {
                    measure.start.get_or_insert(at);
                }
                PathEvent::Line { from, to } => {
                    let line = LineSegment { from, to };
                    push(
                        pieces,
                        Curve::Line(line),
                        &line,
                        0.0..1.0,
                        follows_quadratic,
                    );
                }
                PathEvent::End { last, first, close } => {
                    if close {
                        let line = LineSegment {
                            from: last,
                            to: first,
                        };
                        push(pieces, Curve::Line(line), &line, 0.0..1.0, false);
                    }
                }
                PathEvent::Quadratic { from, ctrl, to } => {
                    let curve = QuadraticBezierSegment { from, ctrl, to };
                    curve.for_each_flattened_with_t(tolerance, &mut |chord, t| {
                        push(pieces, Curve::Quadratic(curve), chord, t, false);
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
                        push(pieces, Curve::Cubic(curve), chord, t, false);
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
