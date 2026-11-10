//! Equirectangular environment maps: read from untrusted Radiance `.hdr` or OpenEXR `.exr` bytes
//! by [`decode`], and prefiltered by [`prefilter`] into the light they give a scene.
//!
//! A map's centre looks along -Z, with +X to its right, +Y along its top edge and -Y along its
//! bottom one. Its texels hold linear radiance.
//!
//! Reading checks a file's size against the limits below before decoding any pixel, decodes it
//! row by row into a map shrunk to within [`MAX_MAP_SIDE`] and [`MAX_MAP_PIXELS`], allocates
//! what it keeps fallibly, and fails with an [`EnvironmentError`] rather than panicking. Every
//! texel it keeps is finite and within `0..=`[`MAX_RADIANCE`]. OpenEXR needs the `exr` feature.

#[cfg(feature = "exr")]
mod exr;
mod hdr;
mod prefilter;
#[cfg(test)]
mod tests;

use std::f32::consts::PI;
use std::fmt;

use glam::{Vec2, Vec3};

pub use prefilter::{
    BRDF_TABLE_SIZE, Prefiltered, SPECULAR_LEVELS, SPECULAR_SIDE, brdf_table, prefilter,
};

/// The largest file read.
pub const MAX_ENVIRONMENT_BYTES: usize = 256 << 20;
/// The most pixels across either side of a file.
pub const MAX_SOURCE_SIDE: u32 = 8192;
/// The most pixels a file may hold.
pub const MAX_SOURCE_PIXELS: u64 = 8192 * 4096;
/// The most texels across either side of a decoded map; larger files are shrunk by halves.
pub const MAX_MAP_SIDE: u32 = 4096;
/// The most texels a decoded map holds.
pub const MAX_MAP_PIXELS: u64 = 4096 * 2048;
/// The brightest a texel is kept, so that sums over a map stay finite.
pub const MAX_RADIANCE: f32 = 1e9;
/// The largest value a half float holds, which texels are clamped to for the GPU.
const HALF_MAX: f32 = 65504.0;

/// Why an environment map can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentError {
    /// Not a well-formed file, or one whose data contradicts itself.
    Invalid(String),
    /// A well-formed file that needs something this reader doesn't do.
    Unsupported(String),
    /// A file larger than the limits allow.
    TooLarge(String),
}

impl fmt::Display for EnvironmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(details) | Self::Unsupported(details) | Self::TooLarge(details) => {
                f.write_str(details)
            }
        }
    }
}

impl std::error::Error for EnvironmentError {}

fn invalid(details: impl Into<String>) -> EnvironmentError {
    EnvironmentError::Invalid(details.into())
}

fn unsupported(details: impl Into<String>) -> EnvironmentError {
    EnvironmentError::Unsupported(details.into())
}

/// Reads the Radiance `.hdr` or OpenEXR `.exr` map in `bytes`.
pub fn decode(bytes: &[u8]) -> Result<Equirect, EnvironmentError> {
    if bytes.len() > MAX_ENVIRONMENT_BYTES {
        return Err(EnvironmentError::TooLarge(format!(
            "the environment map is larger than the {} MiB it may be",
            MAX_ENVIRONMENT_BYTES >> 20
        )));
    }
    if hdr::is_hdr(bytes) {
        return hdr::decode(bytes);
    }
    if is_exr(bytes) {
        #[cfg(feature = "exr")]
        return exr::decode(bytes);
        #[cfg(not(feature = "exr"))]
        return Err(unsupported("this build of Lumen can't read OpenEXR files"));
    }
    Err(unsupported(
        "an environment map must be a Radiance .hdr or OpenEXR .exr file",
    ))
}

/// Whether `head`, the start of a file, begins a Radiance or OpenEXR file.
pub fn is_environment_magic(head: &[u8]) -> bool {
    hdr::is_hdr(head) || is_exr(head)
}

fn is_exr(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x76, 0x2f, 0x31, 0x01])
}

/// An equirectangular map of linear RGB radiance, its rows from the top.
#[derive(Debug, Clone, PartialEq)]
pub struct Equirect {
    width: u32,
    height: u32,
    texels: Vec<Vec3>,
}

/// `value` as a texel keeps it: non-finite and negative values are 0, and brighter ones
/// [`MAX_RADIANCE`].
fn sanitize(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_RADIANCE)
    } else {
        0.0
    }
}

impl Equirect {
    /// A map of `texels`, which must be `width * height` of them, each channel sanitized.
    pub fn new(width: u32, height: u32, texels: Vec<Vec3>) -> Self {
        assert!(width > 0 && height > 0, "a map has texels");
        assert_eq!(texels.len(), width as usize * height as usize);
        let texels = texels
            .into_iter()
            .map(|texel| Vec3::new(sanitize(texel.x), sanitize(texel.y), sanitize(texel.z)))
            .collect();
        Self {
            width,
            height,
            texels,
        }
    }

    /// A map whose texels hold `radiance` of the direction through their centres.
    pub fn from_fn(width: u32, height: u32, mut radiance: impl FnMut(Vec3) -> Vec3) -> Self {
        let texels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| radiance(texel_direction(width, height, x, y)))
            .collect();
        Self::new(width, height, texels)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn texels(&self) -> &[Vec3] {
        &self.texels
    }

    pub fn texel(&self, x: u32, y: u32) -> Vec3 {
        self.texels[(y * self.width + x) as usize]
    }

    /// The map at half its size, as a mip level is, each texel the mean of the up to four it
    /// covers weighted by the solid angle each spans, so that the light it holds is kept.
    pub fn half(&self) -> Self {
        let width = (self.width / 2).max(1);
        let height = (self.height / 2).max(1);
        let mut texels = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            let rows = if self.height == 1 {
                [0, 0]
            } else {
                [y * 2, y * 2 + 1]
            };
            let weights = rows.map(|row| row_solid_angle(self.height, row));
            let total = weights[0] + weights[1];
            for x in 0..width {
                let columns = if self.width == 1 {
                    [0, 0]
                } else {
                    [x * 2, x * 2 + 1]
                };
                let mut sum = Vec3::ZERO;
                for (row, weight) in rows.into_iter().zip(weights) {
                    sum += (self.texel(columns[0], row) + self.texel(columns[1], row)) * weight;
                }
                texels.push(sum / (2.0 * total));
            }
        }
        Self {
            width,
            height,
            texels,
        }
    }

    /// The map halved until neither side is longer than `side`.
    pub fn fit(&self, side: u32) -> Self {
        let fits = |map: &Self| map.width.max(map.height) <= side.max(1);
        if fits(self) {
            return self.clone();
        }
        let mut map = self.half();
        while !fits(&map) {
            map = map.half();
        }
        map
    }

    /// The light the map holds: radiance integrated over the sphere, per channel.
    pub fn power(&self) -> Vec3 {
        let mut total = Vec3::ZERO;
        for y in 0..self.height {
            let row: Vec3 = (0..self.width).map(|x| self.texel(x, y)).sum();
            total += row * row_solid_angle(self.height, y) * (2.0 * PI / self.width as f32);
        }
        total
    }

    /// The texels as RGBA half floats with alpha 1, each clamped to the largest a half float
    /// holds.
    pub fn rgba16f(&self) -> Vec<u16> {
        let one = half::f16::ONE.to_bits();
        self.texels
            .iter()
            .flat_map(|texel| {
                let [r, g, b] = texel
                    .to_array()
                    .map(|c| half::f16::from_f32(c.min(HALF_MAX)));
                [r.to_bits(), g.to_bits(), b.to_bits(), one]
            })
            .collect()
    }
}

/// The direction through the centre of texel `(x, y)` of a `width` by `height` map.
fn texel_direction(width: u32, height: u32, x: u32, y: u32) -> Vec3 {
    direction(Vec2::new(
        (x as f32 + 0.5) / width as f32,
        (y as f32 + 0.5) / height as f32,
    ))
}

/// The share of the sphere's solid angle, over 2π, that row `y` of a map `height` rows tall
/// spans around the whole circle: the difference in the cosine of the angle from +Y across it.
fn row_solid_angle(height: u32, y: u32) -> f32 {
    let top = PI * y as f32 / height as f32;
    let bottom = PI * (y + 1) as f32 / height as f32;
    top.cos() - bottom.cos()
}

/// The direction a map shows at `uv`, from its top left at `(0, 0)`.
pub fn direction(uv: Vec2) -> Vec3 {
    let phi = (uv.x - 0.5) * 2.0 * PI;
    let theta = uv.y * PI;
    Vec3::new(
        theta.sin() * phi.sin(),
        theta.cos(),
        -theta.sin() * phi.cos(),
    )
}

/// Where a map shows `direction`, which must be of unit length.
pub fn uv(direction: Vec3) -> Vec2 {
    Vec2::new(
        0.5 + direction.x.atan2(-direction.z) / (2.0 * PI),
        direction.y.clamp(-1.0, 1.0).acos() / PI,
    )
}

/// A map being decoded, shrunk by a power of two `factor` so that it fits the limits, which
/// pixels are added to one at a time in any order.
struct Shrink {
    factor: u32,
    /// Each source row's weight within the texels it is part of.
    row_weights: Vec<f32>,
    map: Equirect,
}

impl Shrink {
    /// Room for a file of `width` by `height` pixels, if the limits allow it.
    fn new(width: u32, height: u32) -> Result<Self, EnvironmentError> {
        if width == 0 || height == 0 {
            return Err(invalid("the environment map has no pixels"));
        }
        if width > MAX_SOURCE_SIDE
            || height > MAX_SOURCE_SIDE
            || u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS
        {
            return Err(EnvironmentError::TooLarge(format!(
                "the environment map is {width}x{height} pixels, larger than the \
                 {MAX_SOURCE_SIDE} pixels a side and {MAX_SOURCE_PIXELS} pixels in all it may be"
            )));
        }
        let mut factor = 1;
        let fits = |factor: u32| {
            let (w, h) = ((width / factor).max(1), (height / factor).max(1));
            w.max(h) <= MAX_MAP_SIDE && u64::from(w) * u64::from(h) <= MAX_MAP_PIXELS
        };
        while !fits(factor) {
            factor *= 2;
        }
        let (map_width, map_height) = ((width / factor).max(1), (height / factor).max(1));
        let count = map_width as usize * map_height as usize;
        let mut texels = Vec::new();
        texels
            .try_reserve_exact(count)
            .map_err(|_| EnvironmentError::TooLarge("no memory for the environment map".into()))?;
        texels.resize(count, Vec3::ZERO);
        // A side shorter than `factor` gives its one texel all of its pixels.
        let columns = factor.min(width) as f32;
        let row_weights = (0..height)
            .map(|y| {
                let block = y / factor;
                let rows = block * factor..(block * factor + factor).min(height);
                let total: f32 = rows.map(|row| row_solid_angle(height, row)).sum();
                row_solid_angle(height, y) / (total * columns)
            })
            .collect();
        Ok(Self {
            factor,
            row_weights,
            map: Equirect {
                width: map_width,
                height: map_height,
                texels,
            },
        })
    }

    /// Adds the pixel at `(x, y)` of the file, which must be within its size. Pixels past the last
    /// whole block along a side are left out.
    fn add(&mut self, x: u32, y: u32, rgb: [f32; 3]) {
        let (tx, ty) = (x / self.factor, y / self.factor);
        if tx >= self.map.width || ty >= self.map.height {
            return;
        }
        let weight = self.row_weights[y as usize];
        let texel = &mut self.map.texels[(ty * self.map.width + tx) as usize];
        *texel += Vec3::from(rgb.map(sanitize)) * weight;
    }

    fn finish(self) -> Equirect {
        self.map
    }
}
