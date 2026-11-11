//! What an environment map lights a scene with, under the BRDF of the scene shader's lights:
//! GGX distribution, height-correlated Smith visibility and Schlick's Fresnel.
//!
//! Diffuse light is nine spherical-harmonic coefficients of irradiance. Reflections are levels of
//! the map convolved with GGX lobes of rising roughness, viewed along the normal as the split-sum
//! approximation does, and [`brdf_table`] holds the other half of that split. The convolution sums
//! every texel the lobe reaches with its exact weight, rather than sampling the lobe, so a bright
//! sun spreads into a smooth highlight instead of sparkling.

use std::f32::consts::PI;

use glam::{Vec2, Vec3};

use super::{Equirect, row_solid_angle, texel_direction};

/// The most texels across either side of the sharpest reflection level.
pub const SPECULAR_SIDE: u32 = 512;
/// Reflection levels, from roughness 0 to 1 in even steps, as a small map has fewer.
pub const SPECULAR_LEVELS: u32 = 6;
/// The most texels across either side of the map irradiance is integrated from.
const IRRADIANCE_SIDE: u32 = 128;
/// The weight, relative to its peak, where a lobe's convolution stops.
const LOBE_CUTOFF: f32 = 1e-4;
/// Cells of [`brdf_table`] along each axis.
pub const BRDF_TABLE_SIZE: usize = 32;
/// Samples of the lobe integrated for each cell of [`brdf_table`].
const BRDF_SAMPLES: u32 = 1024;

/// A map prefiltered for lighting.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefiltered {
    /// Reflections: level `i`, half the size of the one before it as mip levels are, is the
    /// light reflected by a surface of roughness `i / (levels - 1)`. Level 0 is the map itself.
    pub levels: Vec<Equirect>,
    /// Irradiance over π, the light a white diffuse surface reflects, as coefficients of the
    /// basis [`sh_basis`] gives.
    pub irradiance: [Vec3; 9],
}

impl Prefiltered {
    /// The light a white diffuse surface facing `normal` reflects, as the scene shader
    /// reconstructs it.
    pub fn irradiance_at(&self, normal: Vec3) -> Vec3 {
        let basis = sh_basis(normal);
        self.irradiance
            .iter()
            .zip(basis)
            .map(|(&coefficient, basis)| coefficient * basis)
            .sum::<Vec3>()
            .max(Vec3::ZERO)
    }
}

/// Prefilters `map`, shrunk first to within [`SPECULAR_SIDE`].
pub fn prefilter(map: &Equirect) -> Prefiltered {
    let base = map.fit(SPECULAR_SIDE);
    let mip_count = 32 - base.width.max(base.height).leading_zeros();
    let count = SPECULAR_LEVELS.min(mip_count) as usize;
    let mut pyramid = vec![base];
    while pyramid.len() < count
        || pyramid
            .last()
            .is_some_and(|level| level.width.max(level.height) > IRRADIANCE_SIDE)
    {
        let next = pyramid.last().expect("a level").half();
        pyramid.push(next);
    }
    let irradiance = pyramid
        .iter()
        .find(|level| level.width.max(level.height) <= IRRADIANCE_SIDE)
        .map(irradiance)
        .expect("halving reaches the irradiance size");
    let levels = (0..count)
        .map(|index| {
            if index == 0 || count == 1 {
                pyramid[index].clone()
            } else {
                convolve(&pyramid[index], index as f32 / (count - 1) as f32)
            }
        })
        .collect();
    Prefiltered { levels, irradiance }
}

/// The nine real spherical harmonics of bands 0 to 2 at `d`, in the order the scene shader uses.
pub fn sh_basis(d: Vec3) -> [f32; 9] {
    [
        0.282_095,
        0.488_603 * d.y,
        0.488_603 * d.z,
        0.488_603 * d.x,
        1.092_548 * d.x * d.y,
        1.092_548 * d.y * d.z,
        0.315_392 * (3.0 * d.z * d.z - 1.0),
        1.092_548 * d.x * d.z,
        0.546_274 * (d.x * d.x - d.y * d.y),
    ]
}

/// Projects `map` onto the basis, convolved with the clamped cosine and divided by π
/// (Ramamoorthi and Hanrahan).
fn irradiance(map: &Equirect) -> [Vec3; 9] {
    let mut coefficients = [Vec3::ZERO; 9];
    let column = 2.0 * PI / map.width as f32;
    for y in 0..map.height {
        let solid_angle = row_solid_angle(map.height, y) * column;
        for x in 0..map.width {
            let radiance = map.texel(x, y) * solid_angle;
            let basis = sh_basis(texel_direction(map.width, map.height, x, y));
            for (coefficient, basis) in coefficients.iter_mut().zip(basis) {
                *coefficient += radiance * basis;
            }
        }
    }
    let bands = [1.0, 2.0 / 3.0, 0.25];
    let band = [0, 1, 1, 1, 2, 2, 2, 2, 2];
    std::array::from_fn(|i| coefficients[i] * bands[band[i]])
}

/// The prefilter lobe of `a2`, alpha to the fourth, between directions whose dot product is `c`,
/// relative to its peak: GGX of the half vector weighted by the cosine to the light, for a view
/// along the normal.
fn lobe(c: f32, a2: f32) -> f32 {
    if c <= 0.0 {
        return 0.0;
    }
    let d = (1.0 + c) * 0.5 * (a2 - 1.0) + 1.0;
    c * a2 * a2 / (d * d)
}

/// `source` convolved with the lobe of `roughness` at its own size. Each output row weighs a
/// source row by the same lobe at every column offset, so the weights are worked out once per
/// pair of rows.
fn convolve(source: &Equirect, roughness: f32) -> Equirect {
    let alpha = roughness * roughness;
    let a2 = alpha * alpha;
    // Where the lobe falls to the cutoff; it rises towards c = 1.
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    for _ in 0..32 {
        let middle = 0.5 * (low + high);
        if lobe(middle, a2) < LOBE_CUTOFF {
            low = middle;
        } else {
            high = middle;
        }
    }
    let cutoff = low;
    let reach = cutoff.acos();
    let (width, height) = (source.width as usize, source.height as usize);
    let row_angle = PI / height as f32;
    let thetas: Vec<f32> = (0..height).map(|y| (y as f32 + 0.5) * row_angle).collect();
    let solid_angles: Vec<f32> = (0..height)
        .map(|y| row_solid_angle(source.height, y as u32))
        .collect();
    let offsets: Vec<f32> = (0..=width / 2)
        .map(|k| (2.0 * PI * k as f32 / width as f32).cos())
        .collect();
    let mut texels = vec![Vec3::ZERO; width * height];
    // Per source row in reach: the row and the weight at each column offset up to the farthest.
    let mut rows: Vec<(usize, Vec<f32>)> = Vec::new();
    for (y, &theta) in thetas.iter().enumerate() {
        rows.clear();
        let (sin, cos) = theta.sin_cos();
        for (source_y, &source_theta) in thetas.iter().enumerate() {
            if (source_theta - theta).abs() > reach + row_angle {
                continue;
            }
            let (source_sin, source_cos) = source_theta.sin_cos();
            let weights: Vec<f32> = offsets
                .iter()
                .map(|&offset| {
                    let c = cos * source_cos + sin * source_sin * offset;
                    if c < cutoff {
                        0.0
                    } else {
                        lobe(c, a2) * solid_angles[source_y]
                    }
                })
                .collect();
            let last = weights.iter().rposition(|&weight| weight > 0.0);
            if let Some(last) = last {
                rows.push((source_y, weights[..=last].to_vec()));
            }
        }
        let total: f32 = rows
            .iter()
            .map(|(_, weights)| {
                (0..width)
                    .map(|x| weights.get(x.min(width - x)).copied().unwrap_or(0.0))
                    .sum::<f32>()
            })
            .sum();
        for x in 0..width {
            let mut sum = Vec3::ZERO;
            for (source_y, weights) in &rows {
                let row = &source.texels[source_y * width..(source_y + 1) * width];
                sum += row[x] * weights[0];
                for (k, &weight) in weights.iter().enumerate().skip(1) {
                    // Column offsets past half the width reach the same columns from either side.
                    if 2 * k > width {
                        break;
                    }
                    let left = (x + width - k) % width;
                    let right = (x + k) % width;
                    sum += if 2 * k == width {
                        row[right] * weight
                    } else {
                        (row[left] + row[right]) * weight
                    };
                }
            }
            texels[y * width + x] = sum / total.max(f32::MIN_POSITIVE);
        }
    }
    Equirect {
        width: source.width,
        height: source.height,
        texels,
    }
}

/// The split-sum scale and bias of the reflectance at normal incidence, `f0 * scale + bias`
/// being the light a surface reflects of a uniform environment of radiance 1, for a view at `n·v`
/// of `(i + 0.5) / BRDF_TABLE_SIZE` and a roughness of `j / (BRDF_TABLE_SIZE - 1)`, at
/// `j * BRDF_TABLE_SIZE + i`.
pub fn brdf_table() -> Vec<[f32; 2]> {
    let n = BRDF_TABLE_SIZE;
    (0..n * n)
        .map(|index| {
            let roughness = (index / n) as f32 / (n - 1) as f32;
            let n_dot_v = ((index % n) as f32 + 0.5) / n as f32;
            split_sum(n_dot_v, roughness)
        })
        .collect()
}

fn visibility_smith(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_dot_l * (n_dot_v * n_dot_v * (1.0 - a2) + a2).sqrt();
    let gl = n_dot_v * (n_dot_l * n_dot_l * (1.0 - a2) + a2).sqrt();
    0.5 / (gv + gl).max(1e-5)
}

/// The split sum's scale and bias at `n_dot_v` and `roughness`, integrated over GGX's half
/// vectors at Hammersley points.
fn split_sum(n_dot_v: f32, roughness: f32) -> [f32; 2] {
    let alpha = roughness * roughness;
    let a2 = alpha * alpha;
    let v = Vec3::new((1.0 - n_dot_v * n_dot_v).max(0.0).sqrt(), 0.0, n_dot_v);
    let (mut scale, mut bias) = (0.0, 0.0);
    for i in 0..BRDF_SAMPLES {
        let xi = Vec2::new(
            (i as f32 + 0.5) / BRDF_SAMPLES as f32,
            i.reverse_bits() as f32 / 4_294_967_296.0,
        );
        let cos_h = ((1.0 - xi.y) / (1.0 + (a2 - 1.0) * xi.y)).sqrt();
        let sin_h = (1.0 - cos_h * cos_h).max(0.0).sqrt();
        let phi = 2.0 * PI * xi.x;
        let h = Vec3::new(sin_h * phi.cos(), sin_h * phi.sin(), cos_h);
        let v_dot_h = v.dot(h).max(0.0);
        let l = 2.0 * v_dot_h * h - v;
        if l.z <= 0.0 {
            continue;
        }
        // The BRDF times n·l over the half vector's density.
        let weight = visibility_smith(n_dot_v, l.z, alpha) * 4.0 * v_dot_h * l.z / cos_h;
        let fresnel = (1.0 - v_dot_h).powi(5);
        scale += (1.0 - fresnel) * weight;
        bias += fresnel * weight;
    }
    [scale, bias].map(|sum| sum / BRDF_SAMPLES as f32)
}
