//! Shutter sampling for the analytic motion blur of drawables and the `transform` node.

/// Most taps a motion-blurred layer averages over its shutter.
pub(crate) const MAX_MOTION_TAPS: usize = 32;

/// The largest coordinate, in pixels, that a bounded texture's bounds are worked out for. Past
/// it the shader's single-precision rounding is too coarse to pad for, and the source stays at
/// full frame.
pub(crate) const MAX_BOUNDED_MAGNITUDE: f64 = 1e6;

/// Single-precision epsilons of rounding that bounds are padded by, per unit of the largest
/// coordinate involved.
pub(crate) const ROUNDING_ERROR_FACTOR: f64 = 16.0;
/// Steps per half of the shutter when measuring how far a layer travels, so that rotation
/// is measured along its arc.
const TRAVEL_STEPS: usize = 8;

/// Frames from the frame to either end of the shutter for a shutter `angle` in degrees, or
/// `None` when motion blur is off. The shutter is centred on the frame.
pub(crate) fn shutter_offset(angle: f64) -> Option<f64> {
    (angle > 0.0).then(|| angle.min(360.0) / 720.0)
}

pub(crate) trait Lerp {
    /// `self` at `t` = 0, moving linearly to `other` at `t` = 1.
    fn lerp(&self, other: &Self, t: f64) -> Self;
}

pub(crate) fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub(crate) fn lerp2(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (lerp(a.0, b.0, t), lerp(a.1, b.1, t))
}

/// A transform at the shutter's open, at the frame, and at the shutter's close. Between
/// them it moves linearly, so a rotation sweeps along its arc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shutter<T> {
    pub(crate) open: T,
    pub(crate) frame: T,
    pub(crate) close: T,
}

impl<T: Lerp + PartialEq> Shutter<T> {
    pub(crate) fn is_static(&self) -> bool {
        self.open == self.frame && self.close == self.frame
    }

    /// The transform at `s`, from -1 at the shutter's open through the frame at 0 to 1 at
    /// its close.
    pub(crate) fn at(&self, s: f64) -> T {
        if s < 0.0 {
            self.frame.lerp(&self.open, -s)
        } else {
            self.frame.lerp(&self.close, s)
        }
    }

    /// Transforms at the middle of equal parts of the shutter, about one per canvas pixel
    /// that the farthest travelling of `points` moves. `to_canvas` maps a point under a
    /// transform.
    pub(crate) fn taps(
        &self,
        points: &[[f64; 2]],
        to_canvas: impl Fn(&T, [f64; 2]) -> [f64; 2],
    ) -> Vec<T> {
        let steps: Vec<T> = (0..=2 * TRAVEL_STEPS)
            .map(|step| self.at(step as f64 / TRAVEL_STEPS as f64 - 1.0))
            .collect();
        let travel = points
            .iter()
            .map(|&point| {
                steps
                    .windows(2)
                    .map(|pair| {
                        let [a, b] = [&pair[0], &pair[1]].map(|step| to_canvas(step, point));
                        (b[0] - a[0]).hypot(b[1] - a[1])
                    })
                    .sum::<f64>()
            })
            .fold(0.0, f64::max);
        let count = (travel.ceil().min(MAX_MOTION_TAPS as f64) as usize).max(2);
        (0..count)
            .map(|tap| self.at((2 * tap + 1) as f64 / count as f64 - 1.0))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Lerp for f64 {
        fn lerp(&self, other: &Self, t: f64) -> Self {
            lerp(*self, *other, t)
        }
    }

    #[test]
    fn shutter_angle_maps_to_frames_either_side() {
        assert_eq!(shutter_offset(0.0), None);
        assert_eq!(shutter_offset(f64::NAN), None);
        assert_eq!(shutter_offset(180.0), Some(0.25));
        assert_eq!(shutter_offset(720.0), Some(0.5));
    }

    #[test]
    fn takes_a_tap_per_pixel_of_travel_within_limits() {
        // An offset in x, moving 10 pixels over the shutter, then 1000.
        let shutter = |distance: f64| Shutter {
            open: -distance / 2.0,
            frame: 0.0,
            close: distance / 2.0,
        };
        let shift = |offset: &f64, point: [f64; 2]| [point[0] + offset, point[1]];
        let taps = shutter(10.0).taps(&[[0.0, 0.0]], shift);
        assert_eq!(taps.len(), 10);
        assert_eq!(taps[0], -4.5);
        assert_eq!(taps[9], 4.5);
        assert_eq!(shutter(0.1).taps(&[[0.0, 0.0]], shift).len(), 2);
        assert_eq!(
            shutter(1000.0).taps(&[[0.0, 0.0]], shift).len(),
            MAX_MOTION_TAPS
        );
    }
}
