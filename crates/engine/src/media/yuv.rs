//! 8-bit YCbCr to RGBA conversion on the CPU, and the matrix assumed for untagged video.

/// The YCbCr matrix a video's samples were encoded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

impl YuvMatrix {
    /// The matrix assumed for video that isn't tagged with one. This is a heuristic, not a fact
    /// about the source: HD is usually BT.709 and SD usually BT.601. It must match `source_matrix` in
    /// `lumen-ffmpeg`, so untagged video looks the same in the preview and in a render.
    pub fn untagged(height: u32) -> Self {
        if height >= 720 {
            Self::Bt709
        } else {
            Self::Bt601
        }
    }

    fn kr_kb(self) -> (f32, f32) {
        match self {
            Self::Bt601 => (0.299, 0.114),
            Self::Bt709 => (0.2126, 0.0722),
            Self::Bt2020 => (0.2627, 0.0593),
        }
    }
}

/// One plane of a frame inside a shared buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YuvPlane {
    pub offset: usize,
    pub stride: usize,
}

/// How a frame's planes are laid out. Chroma is subsampled by two in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvLayout {
    /// Luma, then interleaved Cb and Cr.
    Nv12 { y: YuvPlane, uv: YuvPlane },
    /// Luma, Cb and Cr, with an optional alpha plane at full size.
    I420 {
        y: YuvPlane,
        u: YuvPlane,
        v: YuvPlane,
        a: Option<YuvPlane>,
    },
}

const FRACTION_BITS: u32 = 16;
const ONE: f32 = (1 << FRACTION_BITS) as f32;
const ROUND: i32 = 1 << (FRACTION_BITS - 1);

/// Converts a `width` by `height` frame to tightly packed, straight-alpha RGBA.
pub fn yuv_to_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    layout: YuvLayout,
    matrix: YuvMatrix,
    full_range: bool,
) -> Result<Vec<u8>, String> {
    let (w, h) = (width as usize, height as usize);
    let (chroma_w, chroma_h) = (w.div_ceil(2), h.div_ceil(2));
    let check = |plane: YuvPlane, row_bytes: usize, rows: usize, name: &str| {
        let fits = rows == 0
            || (plane.stride >= row_bytes
                && plane
                    .stride
                    .checked_mul(rows - 1)
                    .and_then(|bytes| bytes.checked_add(plane.offset + row_bytes))
                    .is_some_and(|end| end <= data.len()));
        if fits {
            Ok(())
        } else {
            Err(format!("{name} plane does not fit the buffer"))
        }
    };
    match layout {
        YuvLayout::Nv12 { y, uv } => {
            check(y, w, h, "Y")?;
            check(uv, chroma_w * 2, chroma_h, "UV")?;
        }
        YuvLayout::I420 { y, u, v, a } => {
            check(y, w, h, "Y")?;
            check(u, chroma_w, chroma_h, "U")?;
            check(v, chroma_w, chroma_h, "V")?;
            if let Some(a) = a {
                check(a, w, h, "A")?;
            }
        }
    }

    let (kr, kb) = matrix.kr_kb();
    let kg = 1.0 - kr - kb;
    let (y_scale, y_offset, c_scale) = if full_range {
        (1.0, 0, 1.0)
    } else {
        (255.0 / 219.0, 16, 255.0 / 224.0)
    };
    let fixed = |value: f32| (value * ONE).round() as i32;
    let y_k = fixed(y_scale);
    let r_cr = fixed(2.0 * (1.0 - kr) * c_scale);
    let b_cb = fixed(2.0 * (1.0 - kb) * c_scale);
    let g_cb = fixed(-2.0 * (1.0 - kb) * kb / kg * c_scale);
    let g_cr = fixed(-2.0 * (1.0 - kr) * kr / kg * c_scale);
    let channel = |value: i32| ((value + ROUND) >> FRACTION_BITS).clamp(0, 255) as u8;

    let mut rgba = vec![0; w * h * 4];
    for (row, out) in rgba.chunks_exact_mut(w * 4).enumerate() {
        for (col, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (cx, cy) = (col / 2, row / 2);
            let (luma, cb, cr, alpha) = match layout {
                YuvLayout::Nv12 { y, uv } => {
                    let chroma = uv.offset + cy * uv.stride + cx * 2;
                    (
                        data[y.offset + row * y.stride + col],
                        data[chroma],
                        data[chroma + 1],
                        u8::MAX,
                    )
                }
                YuvLayout::I420 { y, u, v, a } => (
                    data[y.offset + row * y.stride + col],
                    data[u.offset + cy * u.stride + cx],
                    data[v.offset + cy * v.stride + cx],
                    a.map_or(u8::MAX, |a| data[a.offset + row * a.stride + col]),
                ),
            };
            let luma = (i32::from(luma) - y_offset) * y_k;
            let (cb, cr) = (i32::from(cb) - 128, i32::from(cr) - 128);
            pixel[0] = channel(luma + r_cr * cr);
            pixel[1] = channel(luma + g_cb * cb + g_cr * cr);
            pixel[2] = channel(luma + b_cb * cb);
            pixel[3] = alpha;
        }
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nv12(width: u32, height: u32, yuv: [u8; 3]) -> (Vec<u8>, YuvLayout) {
        let (w, h) = (width as usize, height as usize);
        let mut data = vec![yuv[0]; w * h];
        for _ in 0..w.div_ceil(2) * h.div_ceil(2) {
            data.extend([yuv[1], yuv[2]]);
        }
        let layout = YuvLayout::Nv12 {
            y: YuvPlane {
                offset: 0,
                stride: w,
            },
            uv: YuvPlane {
                offset: w * h,
                stride: w.div_ceil(2) * 2,
            },
        };
        (data, layout)
    }

    #[test]
    fn untagged_video_is_bt709_from_720_rows() {
        assert_eq!(YuvMatrix::untagged(719), YuvMatrix::Bt601);
        assert_eq!(YuvMatrix::untagged(720), YuvMatrix::Bt709);
    }

    #[test]
    fn converts_limited_range_with_each_matrix() {
        // Red encoded as limited-range BT.709 by FFmpeg; BT.601 reads the same samples differently.
        let (data, layout) = nv12(3, 3, [76, 109, 211]);
        let bt709 = yuv_to_rgba(&data, 3, 3, layout, YuvMatrix::Bt709, false).unwrap();
        let bt601 = yuv_to_rgba(&data, 3, 3, layout, YuvMatrix::Bt601, false).unwrap();
        assert_eq!(&bt709[..4], &[219, 30, 30, 255]);
        assert_eq!(&bt601[..4], &[202, 10, 32, 255]);
    }

    #[test]
    fn converts_full_range() {
        // The red above after Safari expands it to full range.
        let (data, layout) = nv12(2, 2, [70, 106, 222]);
        let rgba = yuv_to_rgba(&data, 2, 2, layout, YuvMatrix::Bt709, true).unwrap();
        assert_eq!(&rgba[..4], &[218, 30, 29, 255]);
    }

    #[test]
    fn rejects_planes_outside_the_buffer() {
        let (data, layout) = nv12(4, 4, [128, 128, 128]);
        assert!(yuv_to_rgba(&data[..20], 4, 4, layout, YuvMatrix::Bt709, false).is_err());
    }
}
