//! Fractal noise against an `f64` reference of its definition, and the properties animation relies
//! on: the same pixels for the same params, a loop every turn of evolution, and smooth steps.

use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

const SIZE: (u32, u32) = (61, 47);

fn noise(params: Value) -> Scene {
    scene(
        SIZE,
        json!([{ "id": 1, "type": "fractal_noise", "params": params }]),
    )
}

fn render_noise(params: Value) -> Frame {
    let gpu = gpu();
    render(
        &mut gpu.renderer(),
        &noise(params).into(),
        0.0,
        &TestMedia::default(),
    )
}

fn hash(value: u32) -> u32 {
    let mut h = value;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// The noise's definition, worked in `f64` from its params.
struct Reference {
    scale: f64,
    octaves: u32,
    seed: i64,
    evolution: f64,
    turbulent: bool,
    contrast: f64,
    brightness: f64,
    offset: [f64; 2],
}

impl Reference {
    fn params(&self) -> Value {
        json!({
            "scale": self.scale, "octaves": self.octaves, "seed": self.seed,
            "evolution": self.evolution, "mode": if self.turbulent { "turbulent" } else { "smooth" },
            "contrast": self.contrast, "brightness": self.brightness,
            "offset_x": self.offset[0], "offset_y": self.offset[1],
        })
    }

    /// 2D gradient noise whose lattice gradient is one of 16 directions, each turning at 1, -1, 2
    /// or -2 times the evolution.
    fn gradient_noise(&self, seed: u32, point: [f64; 2]) -> f64 {
        let whole = point.map(f64::floor);
        let f = [point[0] - whole[0], point[1] - whole[1]];
        let cell = whole.map(|value| value as i64 as u32);
        let corner = |dx: u32, dy: u32| {
            let index =
                hash(hash(cell[0].wrapping_add(dx) ^ seed) ^ cell[1].wrapping_add(dy)) >> 26;
            let speed = [1.0, -1.0, 2.0, -2.0][(index / 16) as usize];
            let turns = f64::from(index % 16) / 16.0 + speed * self.evolution.rem_euclid(1.0);
            let angle = std::f64::consts::TAU * turns;
            angle.cos() * (f[0] - f64::from(dx)) + angle.sin() * (f[1] - f64::from(dy))
        };
        let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        let [u, v] = f.map(fade);
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let value = lerp(
            lerp(corner(0, 0), corner(1, 0), u),
            lerp(corner(0, 1), corner(1, 1), u),
            v,
        );
        (value * std::f64::consts::SQRT_2).clamp(-1.0, 1.0)
    }

    /// The grey level at canvas pixel `(x, y)`.
    fn shade(&self, x: u32, y: u32) -> f64 {
        let total: f64 = (0..self.octaves)
            .map(|octave| 0.5f64.powi(octave as i32))
            .sum();
        let mut sum = 0.0;
        for octave in 0..self.octaves {
            let seed = hash(self.seed as u32 ^ hash(octave.wrapping_add(0x9e37_79b9)));
            let shift = [1, 2].map(|salt| f64::from(hash(seed ^ salt) >> 8) / f64::from(1 << 24));
            let frequency = f64::from(1u32 << octave) / self.scale;
            let pixel = [f64::from(x), f64::from(y)];
            let point = [0, 1]
                .map(|axis| (pixel[axis] + 0.5 - self.offset[axis]) * frequency + shift[axis]);
            let value = self.gradient_noise(seed, point);
            let weight = 0.5f64.powi(octave as i32) / total;
            sum += weight
                * if self.turbulent {
                    value.abs()
                } else {
                    value * 0.5
                };
        }
        let level = if self.turbulent { sum } else { sum + 0.5 };
        ((level - 0.5) * self.contrast + 0.5 + self.brightness).clamp(0.0, 1.0)
    }
}

#[test]
fn noise_matches_its_definition() {
    let rows = [
        Reference {
            scale: 64.0,
            octaves: 4,
            seed: 0,
            evolution: 0.0,
            turbulent: false,
            contrast: 1.0,
            brightness: 0.0,
            offset: [0.0, 0.0],
        },
        Reference {
            scale: 13.5,
            octaves: 6,
            seed: -7,
            evolution: 0.37,
            turbulent: true,
            contrast: 1.8,
            brightness: 0.1,
            offset: [5.25, -19.0],
        },
        Reference {
            scale: 211.0,
            octaves: 8,
            seed: 123_456_789,
            evolution: 1234.6,
            turbulent: false,
            contrast: 2.5,
            brightness: -0.05,
            offset: [3.0e6, -7.5e5],
        },
    ];
    for row in rows {
        let frame = render_noise(row.params());
        assert_eq!((frame.width, frame.height), SIZE);
        let mut differing = 0;
        for y in 0..frame.height {
            for x in 0..frame.width {
                let expected = (row.shade(x, y) * 255.0).round() as u8;
                let pixel = frame.pixel(x, y);
                assert!(
                    pixel[..3].iter().all(|&c| c.abs_diff(expected) <= 1) && pixel[3] == 255,
                    "seed {}: pixel ({x}, {y}) is {pixel:?}, expected {expected}",
                    row.seed
                );
                differing += usize::from(pixel[0] != expected);
            }
        }
        // Single precision only moves values across a rounding boundary now and then.
        assert!(
            differing * 50 < (SIZE.0 * SIZE.1) as usize,
            "seed {}: {differing} pixels are off by one",
            row.seed
        );
    }
}

#[test]
fn noise_is_deterministic_and_loops_every_turn() {
    let params = |evolution: f64, seed: i64| json!({ "scale": 17, "octaves": 5, "seed": seed, "evolution": evolution, "mode": "turbulent" });
    let base = render_noise(params(0.2, 3));
    assert_eq!(base.rgba, render_noise(params(0.2, 3)).rgba);
    assert_eq!(base.rgba, render_noise(params(1.2, 3)).rgba);
    assert_eq!(base.rgba, render_noise(params(-4.8, 3)).rgba);
    assert_ne!(base.rgba, render_noise(params(0.7, 3)).rgba);
    assert_ne!(base.rgba, render_noise(params(0.2, 4)).rgba);
}

#[test]
fn noise_changes_smoothly_with_evolution_and_offset() {
    let base = render_noise(json!({ "scale": 20, "octaves": 3 }));
    for (name, params) in [
        (
            "evolution",
            json!({ "scale": 20, "octaves": 3, "evolution": 0.002 }),
        ),
        (
            "offset",
            json!({ "scale": 20, "octaves": 3, "offset_x": 0.1 }),
        ),
    ] {
        let stepped = render_noise(params);
        assert_ne!(base.rgba, stepped.rgba, "{name} changes nothing");
        let jump = base
            .rgba
            .iter()
            .zip(&stepped.rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max();
        assert!(jump <= Some(4), "a small {name} step jumps {jump:?}");
    }
}

#[test]
fn noise_fills_its_own_size_and_moves_with_expressions() {
    let sized = |width: u32, height: u32| {
        scene(
            (40, 30),
            json!([{ "id": 1, "type": "fractal_noise", "params": {
                "width": width, "height": height, "evolution": "=frame / 10"
            } }]),
        )
        .frames(3)
    };
    let opaque = |frame: &Frame| frame.rgba.chunks(4).all(|pixel| pixel[3] == 255);
    check(vec![
        Case::new(
            "canvas sized",
            sized(0, 0),
            Expect::Custom(Box::new(move |frames| {
                assert!(
                    frames
                        .iter()
                        .all(|f| (f.width, f.height) == (40, 30) && opaque(f))
                );
                assert_ne!(frames[0].rgba, frames[2].rgba, "evolution does not animate");
            })),
        )
        .frames([0.0, 1.0, 2.0]),
        Case::new(
            "its own size",
            sized(13, 9),
            Expect::Custom(Box::new(|frames| {
                for frame in frames {
                    assert_eq!(frame.pixel(12, 8)[3], 255);
                    assert_eq!(frame.pixel(13, 0), [0; 4]);
                    assert_eq!(frame.pixel(0, 9), [0; 4]);
                }
            })),
        )
        .frames([0.0, 2.0]),
    ]);
}
