//! Seeded noise and random numbers for expressions.
//!
//! Everything here is built from integer hashing and IEEE-exact `f64` operations (`+`, `-`, `*`,
//! `floor`), so a value is bit-identical on every host.

const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const TWO_POW_NEG_53: f64 = 1.0 / 9_007_199_254_740_992.0;

/// The SplitMix64 output function: a bijective mixer of all 64 bits.
fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(GOLDEN_GAMMA);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash(seed: u64, input: u64) -> u64 {
    mix(mix(seed) ^ input)
}

/// The top 53 bits as a value in `[0, 1)`.
fn unit(hash: u64) -> f64 {
    (hash >> 11) as f64 * TWO_POW_NEG_53
}

/// The bits of `value`, with `-0.0` read as `0.0`.
fn bits(value: f64) -> u64 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

/// Uniform in `[0, 1)`, a pure function of the bits of `seed` and `x`.
pub fn random(seed: f64, x: f64) -> f64 {
    unit(hash(bits(seed), bits(x)))
}

/// Smooth 1D gradient noise in `[-1, 1]` with a quintic fade, continuous in `x`.
///
/// The lattice is shifted by an offset derived from `seed`, so integer inputs don't all land on
/// the zero crossings of gradient noise. `x` is floored to an `i64`, which saturates for
/// magnitudes beyond `2^63`; there `x` has no fractional part and the lattice is flat.
pub fn noise(seed: f64, x: f64) -> f64 {
    let seed = bits(seed);
    let shifted = x + unit(hash(seed, u64::MAX));
    let floor = shifted.floor();
    let t = shifted - floor;
    let cell = floor as i64 as u64;

    let gradient = |cell: u64| unit(hash(seed, cell)) * 2.0 - 1.0;
    let fade = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let left = gradient(cell) * t;
    let right = gradient(cell.wrapping_add(1)) * (t - 1.0);
    ((left + (right - left) * fade) * 2.0).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::{noise, random};

    #[test]
    fn matches_golden_values() {
        for (seed, x, expected) in [
            (0.0, 0.0, 0.6524484863740322),
            (0.0, 1.0, 0.013996059553765816),
            (1.0, 0.0, 0.5527267908469196),
            (7.0, 0.5, 0.7553920660716886),
            (0.0, -3.25, 0.8805929852790575),
        ] {
            assert_eq!(random(seed, x), expected, "random({seed}, {x})");
        }
        for (seed, x, expected) in [
            (0.0, 0.0, 0.17045676972985413),
            (1.0, 0.0, 0.018993593328085617),
            (1.0, 0.5, -0.37396682188384345),
            (3.0, -3.25, -0.15283871421888726),
            (1.0, 1e15, 0.0),
        ] {
            assert_eq!(noise(seed, x), expected, "noise({seed}, {x})");
        }
    }

    #[test]
    fn negative_zero_matches_zero() {
        assert_eq!(random(-0.0, -0.0), random(0.0, 0.0));
        assert_eq!(noise(-0.0, -0.0), noise(0.0, 0.0));
    }

    #[test]
    fn outputs_stay_in_range() {
        let (mut min, mut max) = (f64::MAX, f64::MIN);
        for step in -20_000..20_000 {
            let x = f64::from(step) * 0.0137;
            let seed = f64::from(step % 17);
            let value = random(seed, x);
            assert!((0.0..1.0).contains(&value), "random({seed}, {x}) = {value}");
            let value = noise(seed, x);
            assert!(
                (-1.0..=1.0).contains(&value),
                "noise({seed}, {x}) = {value}"
            );
            (min, max) = (min.min(value), max.max(value));
        }
        assert!(min < -0.6 && max > 0.6, "noise spans {min}..{max}");
    }

    #[test]
    fn noise_is_continuous() {
        let step = 1e-6;
        for index in 0..20_000 {
            let x = f64::from(index) * 0.0123 - 100.0;
            let jump = (noise(5.0, x + step) - noise(5.0, x)).abs();
            assert!(jump < 1e-4, "noise jumps {jump} at {x}");
        }
    }

    #[test]
    fn seeds_give_unrelated_curves() {
        let samples: Vec<f64> = (0..2_000).map(|index| f64::from(index) * 0.37).collect();
        let correlation = |a: f64, b: f64| {
            let (xs, ys): (Vec<f64>, Vec<f64>) =
                samples.iter().map(|&x| (noise(a, x), noise(b, x))).unzip();
            let dot: f64 = xs.iter().zip(&ys).map(|(x, y)| x * y).sum();
            let norm = |values: &[f64]| values.iter().map(|v| v * v).sum::<f64>().sqrt();
            dot / (norm(&xs) * norm(&ys))
        };
        assert!(correlation(1.0, 2.0).abs() < 0.15);
        assert!(correlation(1.0, 1.5).abs() < 0.15);
        assert_ne!(random(1.0, 3.0), random(2.0, 3.0));
    }
}
