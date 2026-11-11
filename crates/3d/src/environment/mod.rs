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

    /// The map at half its size, rounded down, as a mip level is: see [`Resample`].
    pub fn half(&self) -> Self {
        let width = (self.width / 2).max(1);
        let height = (self.height / 2).max(1);
        let mut resample = Resample::new(
            self.width,
            self.height,
            Self {
                width,
                height,
                texels: vec![Vec3::ZERO; width as usize * height as usize],
            },
        );
        for y in 0..self.height {
            for x in 0..self.width {
                resample.add(x, y, self.texel(x, y));
            }
        }
        resample.map
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

/// A source map's texels spread over a map no larger along either side. Each texel of the
/// smaller map is the mean of the source over the part of the sphere it covers, weighted by the
/// solid angle each source texel shares with it, so the light the map holds is kept whatever the
/// two sizes, odd or 1 texel across. Both maps stretch to cover the whole sphere, whatever their
/// aspect.
struct Resample {
    /// Per source column, the up to two columns it overlaps and the share of each it covers.
    columns: Vec<[(u32, f32); 2]>,
    /// Per source row, the up to two rows it overlaps and the share of each one's solid angle it
    /// covers.
    rows: Vec<[(u32, f32); 2]>,
    map: Equirect,
}

impl Resample {
    /// Spreads a `width` by `height` source over `map`, which must be zero and no larger.
    fn new(width: u32, height: u32, map: Equirect) -> Self {
        let pi = std::f64::consts::PI;
        Self {
            columns: overlaps(width, map.width, |a, b| b - a),
            rows: overlaps(height, map.height, |a, b| (pi * a).cos() - (pi * b).cos()),
            map,
        }
    }

    /// Adds source texel `(x, y)`, which must be within the source's size.
    fn add(&mut self, x: u32, y: u32, radiance: Vec3) {
        for (row, row_share) in self.rows[y as usize] {
            for (column, column_share) in self.columns[x as usize] {
                let texel = &mut self.map.texels[(row * self.map.width + column) as usize];
                *texel += radiance * (row_share * column_share);
            }
        }
    }
}

/// For each of `source` texels along an axis from 0 to 1, the up to two of `target` texels, no
/// more than `source`, that it overlaps, and the share of each one's `extent` that it covers.
/// The second is a zero share of the first when it overlaps only one.
fn overlaps(source: u32, target: u32, extent: impl Fn(f64, f64) -> f64) -> Vec<[(u32, f32); 2]> {
    debug_assert!(target <= source);
    // Edges in units of 1 / (source * target), so they are exact.
    let (s, t) = (u64::from(source), u64::from(target));
    let unit = (s * t) as f64;
    (0..s)
        .map(|i| {
            let (start, end) = (i * t, (i + 1) * t);
            let share = |j: u64| {
                let covered = extent(
                    start.max(j * s) as f64 / unit,
                    end.min((j + 1) * s) as f64 / unit,
                );
                let whole = extent(j as f64 / t as f64, (j + 1) as f64 / t as f64);
                (j as u32, (covered / whole) as f32)
            };
            let (first, last) = (start / s, (end - 1) / s);
            let first_share = share(first);
            let last_share = if last == first {
                (first_share.0, 0.0)
            } else {
                share(last)
            };
            [first_share, last_share]
        })
        .collect()
}

/// A map being decoded, shrunk by a power of two so that it fits the limits, which pixels are
/// added to one at a time in any order.
struct Shrink(Resample);

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
        let map = Equirect {
            width: map_width,
            height: map_height,
            texels,
        };
        Ok(Self(Resample::new(width, height, map)))
    }

    /// Adds the pixel at `(x, y)` of the file, which must be within its size.
    fn add(&mut self, x: u32, y: u32, rgb: [f32; 3]) {
        self.0.add(x, y, Vec3::from(rgb.map(sanitize)));
    }

    fn finish(self) -> Equirect {
        self.0.map
    }
}
