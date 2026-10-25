//! Easing curves, cubic Béziers and springs. All are pure functions of their inputs.

use super::spec::BuiltinFn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    Linear,
    InQuad,
    OutQuad,
    InOutQuad,
    InCubic,
    OutCubic,
    InOutCubic,
    InExpo,
    OutExpo,
    InOutExpo,
    InBack,
    OutBack,
    InOutBack,
}

const BACK_C1: f64 = 1.70158;
const BACK_C2: f64 = BACK_C1 * 1.525;
const BACK_C3: f64 = BACK_C1 + 1.0;

impl Easing {
    pub const ALL: [Self; 13] = [
        Self::Linear,
        Self::InQuad,
        Self::OutQuad,
        Self::InOutQuad,
        Self::InCubic,
        Self::OutCubic,
        Self::InOutCubic,
        Self::InExpo,
        Self::OutExpo,
        Self::InOutExpo,
        Self::InBack,
        Self::OutBack,
        Self::InOutBack,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::InQuad => "in_quad",
            Self::OutQuad => "out_quad",
            Self::InOutQuad => "in_out_quad",
            Self::InCubic => "in_cubic",
            Self::OutCubic => "out_cubic",
            Self::InOutCubic => "in_out_cubic",
            Self::InExpo => "in_expo",
            Self::OutExpo => "out_expo",
            Self::InOutExpo => "in_out_expo",
            Self::InBack => "in_back",
            Self::OutBack => "out_back",
            Self::InOutBack => "in_out_back",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|easing| easing.name() == name)
    }

    /// The easing a builtin such as `out_expo(t)` applies. `linear(a, b, t)` is not an easing.
    pub fn for_builtin(builtin: BuiltinFn) -> Option<Self> {
        Some(match builtin {
            BuiltinFn::InQuad => Self::InQuad,
            BuiltinFn::OutQuad => Self::OutQuad,
            BuiltinFn::InOutQuad => Self::InOutQuad,
            BuiltinFn::InCubic => Self::InCubic,
            BuiltinFn::OutCubic => Self::OutCubic,
            BuiltinFn::InOutCubic => Self::InOutCubic,
            BuiltinFn::InExpo => Self::InExpo,
            BuiltinFn::OutExpo => Self::OutExpo,
            BuiltinFn::InOutExpo => Self::InOutExpo,
            BuiltinFn::InBack => Self::InBack,
            BuiltinFn::OutBack => Self::OutBack,
            BuiltinFn::InOutBack => Self::InOutBack,
            _ => return None,
        })
    }

    /// Every easing maps 0 to 0 and 1 to 1 exactly, so tweens land on their endpoints.
    pub fn apply(self, t: f64) -> f64 {
        if t == 0.0 || t == 1.0 {
            return t;
        }
        match self {
            Self::Linear => t,
            Self::InQuad => t * t,
            Self::OutQuad => 1.0 - (1.0 - t).powi(2),
            Self::InOutQuad if t < 0.5 => 2.0 * t * t,
            Self::InOutQuad => 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0,
            Self::InCubic => t.powi(3),
            Self::OutCubic => 1.0 - (1.0 - t).powi(3),
            Self::InOutCubic if t < 0.5 => 4.0 * t.powi(3),
            Self::InOutCubic => 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0,
            Self::InExpo => 2f64.powf(10.0 * t - 10.0),
            Self::OutExpo => 1.0 - 2f64.powf(-10.0 * t),
            Self::InOutExpo if t < 0.5 => 2f64.powf(20.0 * t - 10.0) / 2.0,
            Self::InOutExpo => (2.0 - 2f64.powf(-20.0 * t + 10.0)) / 2.0,
            Self::InBack => BACK_C3 * t.powi(3) - BACK_C1 * t * t,
            Self::OutBack => 1.0 + BACK_C3 * (t - 1.0).powi(3) + BACK_C1 * (t - 1.0).powi(2),
            Self::InOutBack if t < 0.5 => {
                (2.0 * t).powi(2) * ((BACK_C2 + 1.0) * 2.0 * t - BACK_C2) / 2.0
            }
            Self::InOutBack => {
                ((2.0 * t - 2.0).powi(2) * ((BACK_C2 + 1.0) * (2.0 * t - 2.0) + BACK_C2) + 2.0)
                    / 2.0
            }
        }
    }
}

/// CSS `cubic-bezier(x1, y1, x2, y2)` at progress `t`, solved like WebKit's `UnitBezier`.
pub fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    const EPSILON: f64 = 1e-7;
    let coefficients = |p1: f64, p2: f64| {
        let c = 3.0 * p1;
        let b = 3.0 * (p2 - p1) - c;
        (1.0 - c - b, b, c)
    };
    let (ax, bx, cx) = coefficients(x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    let (ay, by, cy) = coefficients(y1, y2);
    let sample_x = |s: f64| ((ax * s + bx) * s + cx) * s;
    let x = t.clamp(0.0, 1.0);

    let mut s = x;
    for _ in 0..8 {
        let error = sample_x(s) - x;
        if error.abs() < EPSILON {
            return ((ay * s + by) * s + cy) * s;
        }
        let slope = (3.0 * ax * s + 2.0 * bx) * s + cx;
        if slope.abs() < 1e-6 {
            break;
        }
        s -= error / slope;
    }

    let (mut low, mut high) = (0.0, 1.0);
    s = x;
    for _ in 0..64 {
        let sampled = sample_x(s);
        if (sampled - x).abs() < EPSILON {
            break;
        }
        if x > sampled {
            low = s;
        } else {
            high = s;
        }
        s = (low + high) / 2.0;
    }
    ((ay * s + by) * s + cy) * s
}

pub const SPRING_STIFFNESS: f64 = 170.0;
pub const SPRING_DAMPING: f64 = 26.0;

/// Unit step response of a mass-1 damped spring released at rest, `t` seconds after release.
pub fn spring(t: f64, stiffness: f64, damping: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    let w0 = stiffness.sqrt();
    let zeta = damping / (2.0 * w0);
    let decay = zeta * w0;
    if (zeta - 1.0).abs() < 1e-9 {
        return 1.0 - (-w0 * t).exp() * (1.0 + w0 * t);
    }
    if zeta < 1.0 {
        let wd = w0 * (1.0 - zeta * zeta).sqrt();
        return 1.0 - (-decay * t).exp() * ((wd * t).cos() + decay / wd * (wd * t).sin());
    }
    // `e^(-decay t) cosh(wd t)` written as two exponentials so large `t` can't overflow.
    let wd = w0 * (zeta * zeta - 1.0).sqrt();
    let slow = ((wd - decay) * t).exp();
    let fast = (-(wd + decay) * t).exp();
    1.0 - ((slow + fast) + decay / wd * (slow - fast)) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::spec::builtin_for_name;

    #[test]
    fn every_easing_starts_at_zero_and_ends_at_one() {
        for easing in Easing::ALL {
            assert_eq!(easing.apply(0.0), 0.0, "{}", easing.name());
            assert_eq!(easing.apply(1.0), 1.0, "{}", easing.name());
            let near = |t: f64| (easing.apply(t) - t).abs();
            assert!(
                near(1e-9) < 1e-3 && near(1.0 - 1e-9) < 1e-3,
                "{}",
                easing.name()
            );
        }
    }

    #[test]
    fn in_out_easings_are_symmetric_and_back_overshoots() {
        for easing in [
            Easing::InOutQuad,
            Easing::InOutCubic,
            Easing::InOutExpo,
            Easing::InOutBack,
        ] {
            assert!((easing.apply(0.5) - 0.5).abs() < 1e-12, "{}", easing.name());
        }
        assert!(Easing::InBack.apply(0.3) < 0.0);
        assert!(Easing::OutBack.apply(0.7) > 1.0);
    }

    #[test]
    fn named_easings_have_matching_builtins() {
        for easing in Easing::ALL.into_iter().skip(1) {
            let spec = builtin_for_name(easing.name()).unwrap_or_else(|| panic!("{easing:?}"));
            assert_eq!(Easing::for_builtin(spec.builtin), Some(easing));
        }
        assert_eq!(Easing::for_builtin(BuiltinFn::Linear), None);
    }

    #[test]
    fn cubic_bezier_matches_known_curves() {
        for t in [0.0, 0.1, 0.25, 0.5, 0.8, 1.0] {
            assert!((cubic_bezier(0.0, 0.0, 1.0, 1.0, t) - t).abs() < 1e-6);
        }
        // CSS `ease` at x = 0.5.
        assert!((cubic_bezier(0.25, 0.1, 0.25, 1.0, 0.5) - 0.8024).abs() < 1e-3);
        assert_eq!(cubic_bezier(0.42, 0.0, 0.58, 1.0, 0.0), 0.0);
        assert!((cubic_bezier(0.42, 0.0, 0.58, 1.0, 1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn spring_settles_at_one() {
        assert_eq!(spring(0.0, SPRING_STIFFNESS, SPRING_DAMPING), 0.0);
        assert_eq!(spring(-1.0, SPRING_STIFFNESS, SPRING_DAMPING), 0.0);
        for step in 0..=200 {
            let t = 1.5 + f64::from(step) * 0.05;
            let x = spring(t, SPRING_STIFFNESS, SPRING_DAMPING);
            assert!((x - 1.0).abs() < 1e-3, "t = {t}: {x}");
        }
        for (stiffness, damping) in [(100.0, 2.0), (100.0, 400.0)] {
            assert!((spring(60.0, stiffness, damping) - 1.0).abs() < 1e-3);
        }
        assert!(
            spring(0.3, 100.0, 2.0) > 1.0,
            "underdamped springs overshoot"
        );
    }

    #[test]
    fn critically_damped_spring_is_monotonic() {
        let mut previous = 0.0;
        for step in 1..=300 {
            let x = spring(f64::from(step) * 0.01, 100.0, 20.0);
            assert!(x >= previous && x <= 1.0, "{x}");
            previous = x;
        }
    }
}
