//! Animation clips: keyframed node translations, rotations, scales and morph target weights,
//! sampled at a time.

use glam::{Quat, Vec3};

/// Which clip plays and where in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playback {
    /// The clip's index, as [`super::ModelScene::animation`] gives it.
    pub clip: usize,
    /// Seconds into the clip.
    pub seconds: f32,
    /// Wraps `seconds` around the clip's length, rather than holding its last pose past the end.
    pub looped: bool,
}

impl Playback {
    pub(super) fn time(&self, duration: f32) -> f32 {
        if self.looped && duration > 0.0 {
            self.seconds.rem_euclid(duration)
        } else {
            self.seconds
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Interpolation {
    Step,
    Linear,
    /// Values come as in-tangent, value and out-tangent triples.
    CubicSpline,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Values {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
    /// `count` weights a value, for the node's weights from `start` in the scene's.
    Weights {
        values: Vec<f32>,
        start: usize,
        count: usize,
    },
}

/// One node property's keys. Reading checked that `times` are finite and never decrease, and that
/// `values` holds one value per time, or three with cubic-spline interpolation, each of `count`
/// weights for morph target weights.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Channel {
    pub(super) node: usize,
    pub(super) interpolation: Interpolation,
    pub(super) times: Vec<f32>,
    pub(super) values: Values,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Animation {
    pub(super) name: String,
    /// The last key's time, over all channels.
    pub(super) duration: f32,
    pub(super) channels: Vec<Channel>,
}

impl Animation {
    /// Sets the properties the clip animates in `locals`, each node's translation, rotation and
    /// scale, and in `weights`, the nodes' morph target weights, to their values at `time`. Where
    /// two channels target the same property, the later one wins. A sampled value can overflow to
    /// infinity, which `pose` then leaves out.
    pub(super) fn apply(&self, time: f32, locals: &mut [(Vec3, Quat, Vec3)], weights: &mut [f32]) {
        for channel in &self.channels {
            if let Some(local) = locals.get_mut(channel.node) {
                channel.apply(time, local, weights);
            }
        }
    }
}

impl Channel {
    fn apply(&self, time: f32, local: &mut (Vec3, Quat, Vec3), weights: &mut [f32]) {
        let Some(keys) = self.keys(time) else {
            return;
        };
        match &self.values {
            Values::Translation(values) => {
                local.0 = self.sample(|key| values[key], keys, Vec3::lerp);
            }
            Values::Scale(values) => local.2 = self.sample(|key| values[key], keys, Vec3::lerp),
            Values::Rotation(values) => {
                local.1 = unit(self.sample(|key| values[key], keys, Quat::slerp));
            }
            &Values::Weights {
                ref values,
                start,
                count,
            } => {
                let Some(weights) = weights.get_mut(start..start + count) else {
                    return;
                };
                for (target, weight) in weights.iter_mut().enumerate() {
                    *weight = self.sample(
                        |key| values[key * count + target],
                        keys,
                        |a, b, t| a + (b - a) * t,
                    );
                }
            }
        }
    }

    /// The keys either side of `time` and how far it is from the first to the second. Before the
    /// first key, or at a time that isn't a number, both are the first; from the last on, both
    /// are the last.
    fn keys(&self, time: f32) -> Option<Keys> {
        let first = *self.times.first()?;
        let last = self.times.len() - 1;
        if time.is_nan() || time <= first {
            return Some(Keys::at(0));
        }
        if time >= self.times[last] {
            return Some(Keys::at(last));
        }
        let to = self.times.partition_point(|&key| key <= time).min(last);
        let from = to.checked_sub(1)?;
        let span = self.times[to] - self.times[from];
        let t = if span > 0.0 {
            (time - self.times[from]) / span
        } else {
            0.0
        };
        Some(Keys { from, to, t, span })
    }

    /// The value at `keys`, from `values`, which gives each of the channel's values by its
    /// index: one per key, or three with cubic-spline interpolation.
    fn sample<T>(&self, values: impl Fn(usize) -> T, keys: Keys, lerp: impl Fn(T, T, f32) -> T) -> T
    where
        T: Copy + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
    {
        let Keys { from, to, t, span } = keys;
        match self.interpolation {
            Interpolation::Step => values(from),
            Interpolation::Linear => lerp(values(from), values(to), t),
            Interpolation::CubicSpline => {
                let value = |key: usize| values(key * 3 + 1);
                if from == to {
                    return value(from);
                }
                let (t2, t3) = (t * t, t * t * t);
                let out_tangent = values(from * 3 + 2) * span;
                let in_tangent = values(to * 3) * span;
                value(from) * (2.0 * t3 - 3.0 * t2 + 1.0)
                    + out_tangent * (t3 - 2.0 * t2 + t)
                    + value(to) * (-2.0 * t3 + 3.0 * t2)
                    + in_tangent * (t3 - t2)
            }
        }
    }
}

/// `rotation` at unit length, or no rotation when it has no direction or isn't finite.
pub(super) fn unit(rotation: Quat) -> Quat {
    let length = rotation.length();
    if length > 1e-12 && length.is_finite() {
        rotation / length
    } else {
        Quat::IDENTITY
    }
}

#[derive(Debug, Clone, Copy)]
struct Keys {
    from: usize,
    to: usize,
    t: f32,
    /// Seconds between the two keys.
    span: f32,
}

impl Keys {
    fn at(key: usize) -> Self {
        Self {
            from: key,
            to: key,
            t: 0.0,
            span: 0.0,
        }
    }
}
