//! Video colour beyond SDR BT.709: wide-gamut primaries convert to BT.709 in linear light, and PQ
//! and HLG sources are tone-mapped to SDR with the BT.2390 EETF, 203 nits as SDR white and a
//! 1000-nit peak. Expected values are worked out here from the standards' formulas.

use lumen_engine::gpu::RenderTarget;
use lumen_engine::media::{
    ChromaSubsampling, ColorPrimaries, Rotation, TransferFunction, YuvColor, YuvFormat, YuvFrame,
    YuvLayout, YuvMatrix,
};
use serde_json::json;

use crate::support::{TestMedia, gpu, render_working_space, scene};

const SDR_WHITE: f64 = 203.0;
const HDR_PEAK: f64 = 1000.0;

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

/// SMPTE ST 2084: nits to a PQ code value.
fn pq_encode(nits: f64) -> f64 {
    let y = (nits / 10000.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y)).powf(PQ_M2)
}

/// SMPTE ST 2084: a PQ code value to nits.
fn pq_decode(code: f64) -> f64 {
    let p = code.powf(1.0 / PQ_M2);
    10000.0 * ((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1)
}

/// ARIB STD-B67: an HLG code value to scene light.
fn hlg_decode(code: f64) -> f64 {
    let a: f64 = 0.178_832_77;
    let b = 1.0 - 4.0 * a;
    let c = 0.5 - a * (4.0 * a).ln();
    if code <= 0.5 {
        code * code / 3.0
    } else {
        (((code - c) / a).exp() + b) / 12.0
    }
}

/// Grey HLG scene light on a 1000-nit display: the OOTF with system gamma 1.2.
fn hlg_grey_nits(code: f64) -> f64 {
    HDR_PEAK * hlg_decode(code).powf(1.2)
}

/// ITU-R BT.2390's EETF from a 1000-nit source onto a 203-nit display, as SDR (1 is 203 nits).
fn tone_map(nits: f64) -> f64 {
    let source = pq_encode(HDR_PEAK);
    let max_lum = pq_encode(SDR_WHITE) / source;
    let knee = 1.5 * max_lum - 0.5;
    let e = (pq_encode(nits) / source).min(1.0);
    if e <= knee {
        return nits / SDR_WHITE;
    }
    let t = (e - knee) / (1.0 - knee);
    let mapped = (2.0 * t.powi(3) - 3.0 * t * t + 1.0) * knee
        + (t.powi(3) - 2.0 * t * t + t) * (1.0 - knee)
        + (-2.0 * t.powi(3) + 3.0 * t * t) * max_lum;
    pq_decode(mapped * source) / SDR_WHITE
}

/// Linear light in `0..=1` sRGB-encoded.
fn srgb_encode(linear: f64) -> f64 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// BT.2020 to BT.709 in linear light, as ITU-R BT.2087 publishes it, by rows.
const BT2020_TO_709: [[f64; 3]; 3] = [
    [1.6605, -0.5876, -0.0728],
    [-0.1246, 1.1329, -0.0083],
    [-0.0182, -0.1006, 1.1187],
];

/// Display P3 to BT.709 in linear light, by rows.
const P3_TO_709: [[f64; 3]; 3] = [
    [1.2249, -0.2249, 0.0],
    [-0.0421, 1.0421, 0.0],
    [-0.0196, -0.0786, 1.0983],
];

fn kr_kb(matrix: YuvMatrix) -> (f64, f64) {
    match matrix {
        YuvMatrix::Bt601 => (0.299, 0.114),
        YuvMatrix::Bt709 => (0.2126, 0.0722),
        YuvMatrix::Bt2020 => (0.2627, 0.0593),
    }
}

/// A row of 16-bit full-range 4:4:4 pixels holding encoded R'G'B' `pixels`, converted to YCbCr
/// with `color`'s matrix.
fn frame(pixels: &[[f64; 3]], color: YuvColor) -> YuvFrame {
    let format = YuvFormat {
        width: pixels.len() as u32,
        height: 1,
        subsampling: ChromaSubsampling::Yuv444,
        depth: 16,
        msb_aligned: false,
        color,
        rotation: Rotation::None,
    };
    let (layout, bytes) = YuvLayout::packed(&format, false, false);
    let (kr, kb) = kr_kb(color.matrix);
    let code = |value: f64| ((value * 65535.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes();
    let mut planes = [Vec::new(), Vec::new(), Vec::new()];
    for &[r, g, b] in pixels {
        let y = kr * r + (1.0 - kr - kb) * g + kb * b;
        let cb = (b - y) / (2.0 * (1.0 - kb)) + 32768.0 / 65535.0;
        let cr = (r - y) / (2.0 * (1.0 - kr)) + 32768.0 / 65535.0;
        for (plane, value) in planes.iter_mut().zip([y, cb, cr]) {
            plane.extend(code(value));
        }
    }
    let data = planes.concat();
    assert_eq!(data.len(), bytes);
    YuvFrame {
        format,
        layout,
        data: std::sync::Arc::new(data),
    }
}

fn color(matrix: YuvMatrix, primaries: ColorPrimaries, transfer: TransferFunction) -> YuvColor {
    YuvColor {
        matrix,
        full_range: true,
        primaries,
        transfer,
    }
}

/// The working-space RGB each pixel of `frame` shows as, row by row.
fn shown(frame: YuvFrame) -> Vec<[f64; 3]> {
    let (width, height) = frame.format.display_size();
    let media = TestMedia::default().yuv_video("clip", width, height, frame);
    let composition = scene(
        (width, height),
        json!([{ "id": 1, "type": "media_in", "params": { "kind": "video", "source": "clip" } }]),
    )
    .into();
    render_working_space(&mut gpu().renderer(), &composition, 0.0, &media)
        .into_iter()
        .map(|[r, g, b, a]| {
            assert_eq!(a, 1.0);
            [r, g, b].map(f64::from)
        })
        .collect()
}

/// Each channel is within 0.3% of `expected`, or 0.002 of it near zero: a half float holds about
/// 0.05%, and the shader computes in single precision.
fn assert_near(actual: &[[f64; 3]], expected: &[[f64; 3]], case: &str) {
    let mut failures = Vec::new();
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let close = actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (a - e).abs() <= 0.002_f64.max(0.003 * e.abs()));
        if !close {
            failures.push(format!(
                "{case} pixel {index}: {actual:?}, expected {expected:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn apply(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

#[test]
fn pq_greys_tone_map_with_203_nits_as_sdr_white_and_the_peak_at_one() {
    let nits = [0.0, 26.0, 87.0, 203.0, 500.0, 1000.0, 4000.0];
    let pixels: Vec<_> = nits.iter().map(|&n| [pq_encode(n); 3]).collect();
    let pq = color(
        YuvMatrix::Bt2020,
        ColorPrimaries::Bt2020,
        TransferFunction::Pq,
    );
    let expected: Vec<_> = nits.iter().map(|&n| [tone_map(n); 3]).collect();
    // Below the knee, near 88 nits, light is only scaled; SDR white lands on the shoulder; the
    // peak and anything brighter are SDR white.
    assert!((expected[1][0] - 26.0 / 203.0).abs() < 1e-12);
    assert!((expected[3][0] - 0.7835).abs() < 1e-4, "{}", expected[3][0]);
    assert!((expected[5][0] - 1.0).abs() < 1e-12);
    assert_eq!(expected[6][0], expected[5][0]);
    assert_near(&shown(frame(&pixels, pq)), &expected, "PQ");
}

#[test]
fn hlg_greys_go_through_the_ootf_at_1000_nits_then_the_same_tone_map() {
    let codes = [0.0, 0.25, 0.5, 0.75, 1.0];
    let pixels: Vec<_> = codes.iter().map(|&c| [c; 3]).collect();
    let hlg = color(
        YuvMatrix::Bt2020,
        ColorPrimaries::Bt2020,
        TransferFunction::Hlg,
    );
    let expected: Vec<_> = codes
        .iter()
        .map(|&c| [tone_map(hlg_grey_nits(c)); 3])
        .collect();
    // HLG's reference white, 75%, is 203 nits on a 1000-nit display.
    assert!((hlg_grey_nits(0.75) - 203.0).abs() < 0.2);
    assert_near(&shown(frame(&pixels, hlg)), &expected, "HLG");
}

#[test]
fn hdr_colour_keeps_its_hue_through_the_shoulder() {
    // A BT.709 colour as BT.2020 PQ, bright enough that its brightest channel is past the knee.
    let light = [0.8, 0.4, 0.1].map(|c| c * 600.0);
    let in_2020 = apply(invert(BT2020_TO_709), light);
    let pixels = [in_2020.map(pq_encode)];
    let pq = color(
        YuvMatrix::Bt2020,
        ColorPrimaries::Bt2020,
        TransferFunction::Pq,
    );
    let gain = tone_map(light[0]) / light[0];
    let expected = [light.map(|c| c * gain)];
    let actual = shown(frame(&pixels, pq));
    assert_near(&actual, &expected, "PQ colour");
    let ratios = |[r, g, b]: [f64; 3]| [g / r, b / r];
    let (actual, expected) = (ratios(actual[0]), ratios(expected[0]));
    for (a, e) in actual.iter().zip(expected) {
        assert!(
            (a - e).abs() < 0.005,
            "{actual:?} keeps the ratios {expected:?}"
        );
    }
}

#[test]
fn wide_gamut_primaries_convert_to_bt709_and_clip_at_zero() {
    let cases = [
        ("BT.2020", ColorPrimaries::Bt2020, BT2020_TO_709),
        ("Display P3", ColorPrimaries::DisplayP3, P3_TO_709),
    ];
    for (name, primaries, matrix) in cases {
        // A colour inside BT.709 converts back to itself; the pure primaries are outside it.
        let inside = [0.5, 0.2, 0.1];
        let source = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            apply(invert(matrix), inside),
        ];
        let pixels: Vec<_> = source.iter().map(|rgb| rgb.map(srgb_encode)).collect();
        let sdr = color(YuvMatrix::Bt2020, primaries, TransferFunction::Sdr);
        let expected: Vec<_> = source
            .iter()
            .map(|&rgb| apply(matrix, rgb).map(|c| c.max(0.0)))
            .collect();
        assert_near(&shown(frame(&pixels, sdr)), &expected, name);
    }
}

#[test]
fn dci_p3_white_is_adapted_to_d65_white() {
    let sdr = color(
        YuvMatrix::Bt709,
        ColorPrimaries::DciP3,
        TransferFunction::Sdr,
    );
    assert_near(
        &shown(frame(&[[1.0; 3], [0.5; 3]], sdr)),
        &[[1.0; 3], [srgb_decode(0.5); 3]],
        "DCI-P3",
    );
}

#[test]
fn the_matrix_follows_the_tags() {
    // One encoded colour stored with each matrix reads back as the same colour.
    let encoded = [0.8, 0.3, 0.5];
    for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
        let pixels = [encoded];
        let shown = shown(frame(&pixels, YuvColor::sdr(matrix, true)));
        assert_near(&shown, &[encoded.map(srgb_decode)], &format!("{matrix:?}"));
    }
}

#[test]
fn a_video_whose_colour_tags_are_not_converted_warns_once() {
    let details = "its `log100` transfer is read as SDR, which the renderer doesn't convert";
    let frame = frame(&[[0.5; 3]], YuvColor::sdr(YuvMatrix::Bt709, true));
    let media = TestMedia::default()
        .yuv_video("clip", 1, 1, frame)
        .color_warning("clip", details);
    let composition = scene(
        (1, 1),
        json!([{ "id": 1, "type": "media_in", "params": { "kind": "video", "source": "clip" } }]),
    )
    .frames(2)
    .into();
    let mut renderer = gpu().renderer();
    render_working_space(&mut renderer, &composition, 0.0, &media);
    renderer
        .render(&composition, 1.0, &media, RenderTarget::Owned)
        .expect("frame 1 renders");
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "media.color_unsupported");
    assert_eq!(
        warnings[0].message,
        format!("media_in node 1: video `clip`: {details}")
    );
}

fn srgb_decode(encoded: f64) -> f64 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let c =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    [
        [c(1, 2, 1, 2), -c(0, 2, 1, 2), c(0, 1, 1, 2)],
        [-c(1, 2, 0, 2), c(0, 2, 0, 2), -c(0, 1, 0, 2)],
        [c(1, 2, 0, 1), -c(0, 2, 0, 1), c(0, 1, 0, 1)],
    ]
    .map(|row| row.map(|value| value / det))
}

/// Real files from the FFmpeg CLI, decoded on the CPU: the tags are read, 10-bit samples keep
/// their precision, and an HDR grey is tone-mapped rather than shown as SDR.
#[cfg(feature = "ffmpeg")]
mod files {
    use std::{io::Write, path::Path, process::Command, process::Stdio};

    use lumen_engine::media::{
        ColorPrimaries, FfmpegVideoResolver, MediaFrame, TransferFunction, VideoFrameResolver,
        YuvFrame,
    };

    use super::{hlg_grey_nits, pq_decode, shown, tone_map};

    const WIDTH: usize = 64;
    const HEIGHT: usize = 16;

    /// Encodes one 4:2:0 10-bit frame of grey luma `codes`, one per column, losslessly with the
    /// colour tags `params` sets. `None` when the CLI or libx264 is missing.
    fn grey_clip(directory: &Path, name: &str, codes: &[u16], params: &str) -> Option<YuvFrame> {
        let mut raw = Vec::new();
        for _ in 0..HEIGHT {
            for x in 0..WIDTH {
                raw.extend(codes[x * codes.len() / WIDTH].to_le_bytes());
            }
        }
        raw.extend(512_u16.to_le_bytes().repeat(WIDTH * HEIGHT / 2));
        let path = directory.join(name);
        let mut child = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p10le", "-s"])
            .arg(format!("{WIDTH}x{HEIGHT}"))
            .args(["-i", "-", "-vf"])
            .arg(format!("setparams={params}"))
            .args(["-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv420p10le"])
            .arg(&path)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        let _ = child.stdin.take()?.write_all(&raw);
        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("Unknown encoder"), "{stderr}");
            eprintln!("libx264 unavailable; skipping");
            return None;
        }
        let resolver = FfmpegVideoResolver::open(path.to_string_lossy()).expect("open clip");
        assert_eq!(resolver.color_warning(), None, "{name}");
        let MediaFrame::Yuv(frame) = resolver.frame(0).expect("decode frame") else {
            panic!("{name} decodes to YUV planes");
        };
        assert_eq!(frame.format.depth, 10, "{name}");
        Some((*frame).clone())
    }

    /// The working-space grey at the middle of each of `patches` patches across `frame`.
    fn patch_greys(frame: YuvFrame, patches: usize) -> Vec<f64> {
        let shown = shown(frame);
        (0..patches)
            .map(|patch| shown[(2 * patch + 1) * WIDTH / (2 * patches)][1])
            .collect()
    }

    /// A limited-range 10-bit luma code's normalised value.
    fn normalised(code: u16) -> f64 {
        (f64::from(code) - 64.0) / 876.0
    }

    #[test]
    fn hdr_files_are_tagged_and_their_greys_tone_mapped() {
        let directory = tempfile::tempdir().expect("temp dir");
        // Mid grey (26 nits in BT.2408) and HDR reference white, as PQ codes.
        let pq_codes = [
            64 + (0.380 * 876.0_f64).round() as u16,
            64 + (0.5807 * 876.0_f64).round() as u16,
        ];
        let Some(pq) = grey_clip(
            directory.path(),
            "pq.mp4",
            &pq_codes,
            "color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc:range=tv",
        ) else {
            return;
        };
        assert_eq!(pq.format.color.transfer, TransferFunction::Pq);
        assert_eq!(pq.format.color.primaries, ColorPrimaries::Bt2020);
        let expected: Vec<_> = pq_codes
            .iter()
            .map(|&code| tone_map(pq_decode(normalised(code))))
            .collect();
        let actual = patch_greys(pq, pq_codes.len());
        for (a, e) in actual.iter().zip(&expected) {
            assert!((a - e).abs() <= 0.003 * e, "PQ grey {a}, expected {e}");
        }
        // Mid grey stays 26 nits; HDR reference white read as SDR would be its code, 0.58,
        // decoded with the sRGB curve: 0.30 instead of 0.78.
        assert!((actual[0] - 26.0 / 203.0).abs() < 0.01, "{actual:?}");
        assert!((actual[1] - 0.7835).abs() < 0.01, "{actual:?}");

        let hlg_codes = [
            64 + (0.5 * 876.0_f64).round() as u16,
            64 + (0.75 * 876.0_f64).round() as u16,
        ];
        let hlg = grey_clip(
            directory.path(),
            "hlg.mp4",
            &hlg_codes,
            "color_primaries=bt2020:color_trc=arib-std-b67:colorspace=bt2020nc:range=tv",
        )
        .expect("libx264 encoded the PQ clip");
        assert_eq!(hlg.format.color.transfer, TransferFunction::Hlg);
        let expected: Vec<_> = hlg_codes
            .iter()
            .map(|&code| tone_map(hlg_grey_nits(normalised(code))))
            .collect();
        let actual = patch_greys(hlg, hlg_codes.len());
        for (a, e) in actual.iter().zip(&expected) {
            assert!((a - e).abs() <= 0.003 * e, "HLG grey {a}, expected {e}");
        }
    }

    #[test]
    fn ten_bit_samples_keep_their_precision() {
        let directory = tempfile::tempdir().expect("temp dir");
        // 64 consecutive codes span 16 eight-bit steps, so an 8-bit path would repeat values.
        let codes: Vec<u16> = (400..464).collect();
        let Some(frame) = grey_clip(
            directory.path(),
            "ramp.mp4",
            &codes,
            "color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv",
        ) else {
            return;
        };
        let row = &shown(frame)[..WIDTH];
        for pair in row.windows(2) {
            assert!(
                pair[1][1] > pair[0][1],
                "{:?} rises",
                [pair[0][1], pair[1][1]]
            );
        }
    }
}
