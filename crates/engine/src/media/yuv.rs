//! Video frames as YUV planes, and the colour rule and coefficients that turn them into RGB.

use std::sync::Arc;

/// The YUV→RGB matrix of a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

impl YuvMatrix {
    /// The matrix of a source with no colour tags: BT.709 from 720 lines up, BT.601 below.
    pub fn untagged(height: u32) -> Self {
        if height >= 720 {
            Self::Bt709
        } else {
            Self::Bt601
        }
    }

    /// The luma weights of red and blue.
    fn kr_kb(self) -> (f32, f32) {
        match self {
            Self::Bt601 => (0.299, 0.114),
            Self::Bt709 => (0.2126, 0.0722),
            Self::Bt2020 => (0.2627, 0.0593),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YuvColor {
    pub matrix: YuvMatrix,
    /// Samples span the whole code range rather than the limited (16–235 at 8 bits) one.
    pub full_range: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSubsampling {
    /// Chroma at half width and half height.
    Yuv420,
    /// Chroma at half width.
    Yuv422,
    Yuv444,
}

impl ChromaSubsampling {
    /// How far a luma position shifts right to reach its chroma sample, in x and y.
    fn shifts(self) -> (u32, u32) {
        match self {
            Self::Yuv420 => (1, 1),
            Self::Yuv422 => (1, 0),
            Self::Yuv444 => (0, 0),
        }
    }
}

/// How far a frame turns clockwise to show upright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    None,
    Cw90,
    Cw180,
    Cw270,
}

impl Rotation {
    /// The rotation of a whole number of quarter turns clockwise, in degrees.
    pub fn from_degrees(degrees: u32) -> Option<Self> {
        match degrees % 360 {
            0 => Some(Self::None),
            90 => Some(Self::Cw90),
            180 => Some(Self::Cw180),
            270 => Some(Self::Cw270),
            _ => None,
        }
    }

    fn quarter_turns(self) -> u32 {
        self as u32
    }
}

/// What a frame's samples are, wherever its planes live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YuvFormat {
    /// The coded size, before rotation.
    pub width: u32,
    pub height: u32,
    pub subsampling: ChromaSubsampling,
    /// Significant bits per sample: 8 stores a byte per sample, 9 to 16 two little-endian bytes.
    pub depth: u8,
    /// A two-byte sample's value sits in its high bits, as in P010.
    pub msb_aligned: bool,
    pub color: YuvColor,
    pub rotation: Rotation,
}

impl YuvFormat {
    /// The size the frame shows at, once rotated upright.
    pub fn display_size(&self) -> (u32, u32) {
        match self.rotation {
            Rotation::None | Rotation::Cw180 => (self.width, self.height),
            Rotation::Cw90 | Rotation::Cw270 => (self.height, self.width),
        }
    }

    pub fn chroma_size(&self) -> (u32, u32) {
        let (x, y) = self.subsampling.shifts();
        (self.width.div_ceil(1 << x), self.height.div_ceil(1 << y))
    }

    pub fn bytes_per_sample(&self) -> usize {
        if self.depth > 8 { 2 } else { 1 }
    }
}

/// One plane's place in a frame's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YuvPlane {
    pub offset: usize,
    /// Bytes from the start of one row to the next.
    pub stride: usize,
}

/// A frame's chroma: one plane of Cb, Cr pairs, or a plane of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvChroma<P> {
    Interleaved(P),
    Planar { u: P, v: P },
}

/// A frame's planes, as places in a buffer or as textures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YuvPlanes<P> {
    pub luma: P,
    pub chroma: YuvChroma<P>,
    /// Full size, and always full range.
    pub alpha: Option<P>,
}

pub type YuvLayout = YuvPlanes<YuvPlane>;

/// A plane's size in texels and the samples each texel holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlaneShape {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) channels: u32,
}

impl<P> YuvPlanes<P> {
    /// Each plane paired with its shape in `format`.
    pub(crate) fn map_with_shape<Q>(
        &self,
        format: &YuvFormat,
        mut f: impl FnMut(&P, PlaneShape) -> Q,
    ) -> YuvPlanes<Q> {
        let luma = PlaneShape {
            width: format.width,
            height: format.height,
            channels: 1,
        };
        let (width, height) = format.chroma_size();
        let chroma = |channels| PlaneShape {
            width,
            height,
            channels,
        };
        YuvPlanes {
            luma: f(&self.luma, luma),
            chroma: match &self.chroma {
                YuvChroma::Interleaved(plane) => YuvChroma::Interleaved(f(plane, chroma(2))),
                YuvChroma::Planar { u, v } => YuvChroma::Planar {
                    u: f(u, chroma(1)),
                    v: f(v, chroma(1)),
                },
            },
            alpha: self.alpha.as_ref().map(|plane| f(plane, luma)),
        }
    }

    fn iter(&self) -> impl Iterator<Item = &P> {
        let (first, second) = match &self.chroma {
            YuvChroma::Interleaved(plane) => (plane, None),
            YuvChroma::Planar { u, v } => (u, Some(v)),
        };
        [Some(&self.luma), Some(first), second, self.alpha.as_ref()]
            .into_iter()
            .flatten()
    }
}

impl YuvLayout {
    /// Planes packed back to back without row padding: luma, chroma, then alpha. Returns the layout
    /// and the bytes it spans.
    pub fn packed(format: &YuvFormat, interleaved: bool, alpha: bool) -> (Self, usize) {
        let sample = format.bytes_per_sample();
        let (chroma_width, chroma_height) = format.chroma_size();
        let mut end = 0;
        let mut plane = |width: u32, height: u32, channels: usize| {
            let stride = width as usize * channels * sample;
            let plane = YuvPlane {
                offset: end,
                stride,
            };
            end += stride * height as usize;
            plane
        };
        let luma = plane(format.width, format.height, 1);
        let chroma = if interleaved {
            YuvChroma::Interleaved(plane(chroma_width, chroma_height, 2))
        } else {
            YuvChroma::Planar {
                u: plane(chroma_width, chroma_height, 1),
                v: plane(chroma_width, chroma_height, 1),
            }
        };
        let alpha = alpha.then(|| plane(format.width, format.height, 1));
        (
            Self {
                luma,
                chroma,
                alpha,
            },
            end,
        )
    }
}

/// A decoded frame as YUV planes in one buffer.
#[derive(Debug, Clone)]
pub struct YuvFrame {
    pub format: YuvFormat,
    pub layout: YuvLayout,
    pub data: Arc<Vec<u8>>,
}

impl YuvFrame {
    /// Why the planes can't be read as laid out, if they can't.
    pub fn validate(&self) -> Result<(), String> {
        let format = &self.format;
        if !(8..=16).contains(&format.depth) {
            return Err(format!("a bit depth of {} is unsupported", format.depth));
        }
        if format.width == 0 || format.height == 0 {
            return Err(format!(
                "a frame of {}x{} is empty",
                format.width, format.height
            ));
        }
        let sample = format.bytes_per_sample();
        let shapes = self
            .layout
            .map_with_shape(format, |plane, shape| (*plane, shape));
        for (plane, shape) in shapes.iter() {
            let row = shape.width as usize * shape.channels as usize * sample;
            if u32::try_from(plane.stride).is_err() {
                return Err(format!("a plane's stride of {} is too large", plane.stride));
            }
            if plane.stride < row {
                return Err(format!(
                    "a plane's stride of {} is under its row of {row} bytes",
                    plane.stride
                ));
            }
            let end = plane
                .stride
                .checked_mul(shape.height as usize - 1)
                .and_then(|rows| rows.checked_add(row))
                .and_then(|bytes| bytes.checked_add(plane.offset));
            if end.is_none_or(|end| end > self.data.len()) {
                return Err(format!(
                    "a plane at byte {} runs past the frame's {} bytes",
                    plane.offset,
                    self.data.len()
                ));
            }
        }
        Ok(())
    }
}

/// What `yuv_to_rgba.wgsl` reads a frame with. Normalised luma is `(y - y_offset) * y_scale`,
/// chroma `(c - c_offset) * c_scale` and alpha `a * a_scale`, after a sample shifts right by
/// `sample_shift`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct YuvUniform {
    pub(crate) coded_width: u32,
    pub(crate) coded_height: u32,
    /// Clockwise quarter turns.
    pub(crate) rotation: u32,
    pub(crate) sample_shift: u32,
    pub(crate) chroma_shift_x: u32,
    pub(crate) chroma_shift_y: u32,
    pub(crate) interleaved: u32,
    pub(crate) has_alpha: u32,
    pub(crate) y_offset: f32,
    pub(crate) y_scale: f32,
    pub(crate) c_offset: f32,
    pub(crate) c_scale: f32,
    pub(crate) a_scale: f32,
    pub(crate) r_cr: f32,
    pub(crate) g_cb: f32,
    pub(crate) g_cr: f32,
    pub(crate) b_cb: f32,
    pub(crate) _pad: [f32; 3],
}

impl YuvUniform {
    pub(crate) fn new(format: &YuvFormat, interleaved: bool, has_alpha: bool) -> Self {
        let depth = u32::from(format.depth.clamp(8, 16));
        let max = ((1u32 << depth) - 1) as f32;
        let scale = (1u32 << (depth - 8)) as f32;
        let (y_offset, y_scale, c_offset, c_scale) = if format.color.full_range {
            (0.0, 1.0 / max, (1u32 << (depth - 1)) as f32, 1.0 / max)
        } else {
            (
                16.0 * scale,
                1.0 / (219.0 * scale),
                128.0 * scale,
                1.0 / (224.0 * scale),
            )
        };
        let (kr, kb) = format.color.matrix.kr_kb();
        let kg = 1.0 - kr - kb;
        let (chroma_shift_x, chroma_shift_y) = format.subsampling.shifts();
        let sample_shift = if format.msb_aligned && format.bytes_per_sample() == 2 {
            16 - depth
        } else {
            0
        };
        Self {
            coded_width: format.width,
            coded_height: format.height,
            rotation: format.rotation.quarter_turns(),
            sample_shift,
            chroma_shift_x,
            chroma_shift_y,
            interleaved: u32::from(interleaved),
            has_alpha: u32::from(has_alpha),
            y_offset,
            y_scale,
            c_offset,
            c_scale,
            a_scale: 1.0 / max,
            r_cr: 2.0 * (1.0 - kr),
            g_cb: -2.0 * (1.0 - kb) * kb / kg,
            g_cr: -2.0 * (1.0 - kr) * kr / kg,
            b_cb: 2.0 * (1.0 - kb),
            _pad: [0.0; 3],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(depth: u8, full_range: bool, matrix: YuvMatrix) -> YuvFormat {
        YuvFormat {
            width: 4,
            height: 2,
            subsampling: ChromaSubsampling::Yuv420,
            depth,
            msb_aligned: false,
            color: YuvColor { matrix, full_range },
            rotation: Rotation::None,
        }
    }

    /// RGB in 0–255 for a sample, the way the shader computes it.
    fn rgb(uniform: &YuvUniform, [y, u, v]: [f32; 3]) -> [f32; 3] {
        let y = (y - uniform.y_offset) * uniform.y_scale;
        let cb = (u - uniform.c_offset) * uniform.c_scale;
        let cr = (v - uniform.c_offset) * uniform.c_scale;
        [
            y + uniform.r_cr * cr,
            y + uniform.g_cb * cb + uniform.g_cr * cr,
            y + uniform.b_cb * cb,
        ]
        .map(|channel| channel.clamp(0.0, 1.0) * 255.0)
    }

    fn assert_near(actual: [f32; 3], expected: [u8; 3], case: &str) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - f32::from(expected)).abs() <= 1.0,
                "{case}: {actual} is not {expected}"
            );
        }
    }

    #[test]
    fn untagged_sources_use_bt709_from_720_lines() {
        assert_eq!(YuvMatrix::untagged(576), YuvMatrix::Bt601);
        assert_eq!(YuvMatrix::untagged(719), YuvMatrix::Bt601);
        assert_eq!(YuvMatrix::untagged(720), YuvMatrix::Bt709);
        assert_eq!(YuvMatrix::untagged(2160), YuvMatrix::Bt709);
    }

    #[test]
    fn coefficients_match_reference_conversions() {
        let red = [76.0, 109.0, 211.0];
        let limited_709 = YuvUniform::new(&format(8, false, YuvMatrix::Bt709), true, false);
        assert_near(rgb(&limited_709, red), [219, 30, 30], "limited BT.709");
        let limited_601 = YuvUniform::new(&format(8, false, YuvMatrix::Bt601), true, false);
        assert_near(rgb(&limited_601, red), [202, 10, 32], "limited BT.601");
        let full_709 = YuvUniform::new(&format(8, true, YuvMatrix::Bt709), true, false);
        assert_near(
            rgb(&full_709, [70.0, 106.0, 222.0]),
            [218, 30, 29],
            "full BT.709",
        );
        let ten_bit = YuvUniform::new(&format(10, false, YuvMatrix::Bt709), true, false);
        assert_near(rgb(&ten_bit, red.map(|s| s * 4.0)), [219, 30, 30], "10-bit");
    }

    #[test]
    fn layouts_are_checked_against_the_buffer() {
        let format = format(8, false, YuvMatrix::Bt709);
        let (layout, bytes) = YuvLayout::packed(&format, true, false);
        assert_eq!(bytes, 4 * 2 + 2 * 2);
        let frame = |len| YuvFrame {
            format,
            layout,
            data: Arc::new(vec![0; len]),
        };
        assert_eq!(frame(bytes).validate(), Ok(()));
        assert!(frame(bytes - 1).validate().is_err());
        let mut wide = frame(bytes);
        wide.layout.luma.stride = u32::MAX as usize + 1;
        wide.format.height = 1;
        assert!(wide.validate().is_err());
    }
}
