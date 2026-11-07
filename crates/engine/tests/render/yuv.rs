//! Video frames given as YUV planes, converted to RGBA on the GPU.

use lumen_engine::media::{
    ChromaSubsampling, Rotation, YuvColor, YuvFormat, YuvFrame, YuvLayout, YuvMatrix, YuvPlane,
};
use serde_json::json;

use crate::support::{TestMedia, gpu, render, scene};

/// The coded frame: 2 × 2 blocks of one chroma sample each, three across and two down.
const WIDTH: u32 = 6;
const HEIGHT: u32 = 4;

/// One reference sample, as 8-bit Y, Cb, Cr, and the RGB it converts to.
type Reference = ([u8; 3], [u8; 3]);
const LIMITED_709: Reference = ([76, 109, 211], [219, 30, 30]);
const LIMITED_601: Reference = ([76, 109, 211], [202, 10, 32]);
const FULL_709: Reference = ([70, 106, 222], [218, 30, 29]);

struct Row {
    name: &'static str,
    subsampling: ChromaSubsampling,
    interleaved: bool,
    depth: u8,
    msb_aligned: bool,
    alpha: bool,
    /// The source's colour is already multiplied by its alpha, and the node says so.
    premultiplied: bool,
    matrix: YuvMatrix,
    full_range: bool,
    reference: Reference,
    rotation: Rotation,
    /// The size the source declares, when it isn't the frame's upright size.
    output: Option<(u32, u32)>,
}

fn nv12(name: &'static str) -> Row {
    Row {
        name,
        subsampling: ChromaSubsampling::Yuv420,
        interleaved: true,
        depth: 8,
        msb_aligned: false,
        alpha: false,
        premultiplied: false,
        matrix: YuvMatrix::Bt709,
        full_range: false,
        reference: LIMITED_709,
        rotation: Rotation::None,
        output: None,
    }
}

/// Y, Cb, Cr and alpha at each coded pixel, at 8 bits. The top-left block is the reference
/// sample, the rest neutral grey, one block with a luma per pixel.
fn coded_samples(reference: [u8; 3]) -> Vec<[u8; 4]> {
    let mut pixels = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let corner = ((y % 2) * 2 + x % 2) as usize;
            pixels.push(match (x / 2, y / 2) {
                (0, 0) => [reference[0], reference[1], reference[2], 128],
                (1, 0) => [[16, 235, 126, 60][corner], 128, 128, 255],
                (2, 0) => [235, 128, 128, 255],
                (0, 1) => [16, 128, 128, 255],
                (1, 1) => [126, 128, 128, 64],
                _ => [60, 128, 128, 255],
            });
        }
    }
    pixels
}

/// The premultiplied RGBA each coded pixel shows. A source that is premultiplied already shows
/// its converted colour as it is.
fn coded_expected(row: &Row) -> Vec<[u8; 4]> {
    coded_samples(row.reference.0)
        .into_iter()
        .map(|[y, u, _, a]| {
            let rgb = if u == 128 {
                let grey = if row.full_range {
                    y
                } else {
                    ((f32::from(y) - 16.0) * 255.0 / 219.0).round() as u8
                };
                [grey; 3]
            } else {
                row.reference.1
            };
            let a = if row.alpha { a } else { 255 };
            let premultiply = |c: u8| {
                if row.premultiplied {
                    c
                } else {
                    (f32::from(c) * f32::from(a) / 255.0).round() as u8
                }
            };
            [
                premultiply(rgb[0]),
                premultiply(rgb[1]),
                premultiply(rgb[2]),
                a,
            ]
        })
        .collect()
}

fn frame(row: &Row) -> YuvFrame {
    let format = YuvFormat {
        width: WIDTH,
        height: HEIGHT,
        subsampling: row.subsampling,
        depth: row.depth,
        msb_aligned: row.msb_aligned,
        color: YuvColor {
            matrix: row.matrix,
            full_range: row.full_range,
        },
        rotation: row.rotation,
    };
    let (layout, bytes) = YuvLayout::packed(&format, row.interleaved, row.alpha);
    let mut data = vec![0; bytes];
    let samples = coded_samples(row.reference.0);
    let (shift_x, shift_y) = match row.subsampling {
        ChromaSubsampling::Yuv420 => (1, 1),
        ChromaSubsampling::Yuv422 => (1, 0),
        ChromaSubsampling::Yuv444 => (0, 0),
    };
    let (chroma_width, chroma_height) = format.chroma_size();
    let mut write = |plane: YuvPlane, width: u32, height: u32, channels: &[usize], sub: bool| {
        let mut at = plane.offset;
        for y in 0..height {
            for x in 0..width {
                let (sx, sy) = if sub {
                    (x << shift_x, y << shift_y)
                } else {
                    (x, y)
                };
                let pixel = samples[(sy * WIDTH + sx) as usize];
                for &channel in channels {
                    let value = u16::from(pixel[channel]) << (row.depth - 8);
                    if row.depth == 8 {
                        data[at] = value as u8;
                        at += 1;
                    } else {
                        let shift = if row.msb_aligned { 16 - row.depth } else { 0 };
                        data[at..at + 2].copy_from_slice(&(value << shift).to_le_bytes());
                        at += 2;
                    }
                }
            }
            at = plane.offset + plane.stride * (y as usize + 1);
        }
    };
    write(layout.luma, WIDTH, HEIGHT, &[0], false);
    match layout.chroma {
        lumen_engine::media::YuvChroma::Interleaved(plane) => {
            write(plane, chroma_width, chroma_height, &[1, 2], true);
        }
        lumen_engine::media::YuvChroma::Planar { u, v } => {
            write(u, chroma_width, chroma_height, &[1], true);
            write(v, chroma_width, chroma_height, &[2], true);
        }
    }
    if let Some(alpha) = layout.alpha {
        write(alpha, WIDTH, HEIGHT, &[3], false);
    }
    YuvFrame {
        format,
        layout,
        data: std::sync::Arc::new(data),
    }
}

/// `pixels` turned clockwise, as FFmpeg's rotation metadata asks.
fn rotate(pixels: &[[u8; 4]], rotation: Rotation) -> (u32, u32, Vec<[u8; 4]>) {
    let (w, h) = (WIDTH as usize, HEIGHT as usize);
    let (out_w, out_h) = match rotation {
        Rotation::Cw90 | Rotation::Cw270 => (h, w),
        _ => (w, h),
    };
    let mut out = vec![[0; 4]; pixels.len()];
    for (index, pixel) in pixels.iter().enumerate() {
        let (x, y) = (index % w, index / w);
        let (out_x, out_y) = match rotation {
            Rotation::None => (x, y),
            Rotation::Cw90 => (h - 1 - y, x),
            Rotation::Cw180 => (w - 1 - x, h - 1 - y),
            Rotation::Cw270 => (y, w - 1 - x),
        };
        out[out_y * out_w + out_x] = *pixel;
    }
    (out_w as u32, out_h as u32, out)
}

#[test]
fn yuv_frames_convert_to_premultiplied_rgba() {
    let rows = [
        nv12("nv12, limited BT.709"),
        Row {
            interleaved: false,
            matrix: YuvMatrix::Bt601,
            reference: LIMITED_601,
            ..nv12("i420, limited BT.601")
        },
        Row {
            subsampling: ChromaSubsampling::Yuv422,
            interleaved: false,
            full_range: true,
            reference: FULL_709,
            ..nv12("i422, full BT.709")
        },
        Row {
            subsampling: ChromaSubsampling::Yuv444,
            interleaved: false,
            ..nv12("i444")
        },
        Row {
            interleaved: false,
            depth: 10,
            ..nv12("10-bit i420, LSB-aligned")
        },
        Row {
            depth: 10,
            msb_aligned: true,
            ..nv12("10-bit P010, MSB-aligned")
        },
        Row {
            interleaved: false,
            alpha: true,
            ..nv12("i420 with alpha")
        },
        Row {
            interleaved: false,
            alpha: true,
            premultiplied: true,
            ..nv12("i420 with premultiplied alpha")
        },
        Row {
            rotation: Rotation::Cw90,
            ..nv12("rotated 90")
        },
        Row {
            rotation: Rotation::Cw180,
            ..nv12("rotated 180")
        },
        Row {
            rotation: Rotation::Cw270,
            ..nv12("rotated 270")
        },
        Row {
            output: Some((9, 7)),
            ..nv12("scaled up")
        },
        Row {
            rotation: Rotation::Cw90,
            output: Some((3, 5)),
            ..nv12("rotated 90 and scaled down")
        },
    ];
    let gpu = gpu();
    let mut failures = Vec::new();
    for row in rows {
        let (display_w, display_h, display) = rotate(&coded_expected(&row), row.rotation);
        let (width, height) = row.output.unwrap_or((display_w, display_h));
        let media = TestMedia::default().yuv_video("clip", width, height, frame(&row));
        let alpha = if row.premultiplied {
            "premultiplied"
        } else {
            "straight"
        };
        let composition = scene(
            (width, height),
            json!([{ "id": 1, "type": "media_in", "params": { "kind": "video", "source": "clip", "alpha": alpha } }]),
        )
        .into();
        let shown = render(&mut gpu.renderer(), &composition, 0.0, &media);
        for y in 0..height {
            for x in 0..width {
                let source_x = x * display_w / width;
                let source_y = y * display_h / height;
                let expected = display[(source_y * display_w + source_x) as usize];
                let actual = shown.pixel(x, y);
                if actual
                    .iter()
                    .zip(expected)
                    .any(|(&actual, expected)| actual.abs_diff(expected) > 1)
                {
                    failures.push(format!(
                        "{}: ({x}, {y}) is {actual:?}, expected {expected:?}",
                        row.name
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn raw_rgba_video_is_premultiplied_unless_the_node_says_it_already_is() {
    let gpu = gpu();
    let media = TestMedia::default().raw_video("clip", 1, 1, vec![100, 50, 24, 128]);
    for (alpha, expected) in [
        ("straight", [50, 25, 12, 128]),
        ("premultiplied", [100, 50, 24, 128]),
    ] {
        let composition = scene(
            (1, 1),
            json!([{ "id": 1, "type": "media_in", "params": { "kind": "video", "source": "clip", "alpha": alpha } }]),
        )
        .into();
        let shown = render(&mut gpu.renderer(), &composition, 0.0, &media);
        let actual = shown.pixel(0, 0);
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(&actual, expected)| actual.abs_diff(expected) <= 1),
            "{alpha}: {actual:?}, expected {expected:?}"
        );
    }
}
