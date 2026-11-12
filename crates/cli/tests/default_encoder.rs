#![cfg(target_os = "linux")]

//! Drives the real `lumen` binary on lavapipe, which has no Vulkan Video, so the Linux default
//! encoder and hardware decode both have to fall back. Skips like the engine's GPU tests when
//! lavapipe isn't installed.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use lumen_ffmpeg::{
    CpuVideoFrame, DecodeMode, DecodedVideoFrame, InputContext, MuxedEncoder, PixelFormat,
    VideoDecoder, VideoDecoderConfig, VideoEncoderConfig, YuvVideoFrame,
};
use serde_json::{Value, json};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const FRAMES: u32 = 30;
const ORANGE: [u8; 3] = [255, 128, 32];
const BLUE: [u8; 3] = [32, 128, 255];
const MAX_SAMPLE_ERROR: i32 = 3;

fn lavapipe_icd() -> Option<PathBuf> {
    let icd = fs::read_dir("/usr/share/vulkan/icd.d")
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("lvp_icd"))
        });
    if icd.is_none() {
        eprintln!("lavapipe not installed; skipping lumen fallback test");
    }
    icd
}

fn lumen(icd: &Path, args: &[&str], configure: impl FnOnce(&mut Command)) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lumen"));
    command
        .args(args)
        .env_remove("LUMEN_VIDEO_ENCODER")
        .env_remove("LUMEN_HARDWARE_DECODE")
        .env("VK_ICD_FILENAMES", icd)
        .env("LUMEN_GPU_FORCE_FALLBACK_ADAPTER", "1")
        .env("WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER", "1")
        .env("WGPU_BACKEND", "vulkan")
        .env("WGPU_POWER_PREF", "none")
        .env("NO_COLOR", "1");
    configure(&mut command);
    command.output().expect("run lumen")
}

fn background(paint: [u8; 3]) -> Value {
    json!({
        "id": 1,
        "type": "background",
        "params": {
            "paint": [paint[0], paint[1], paint[2], 255],
            "width": WIDTH,
            "height": HEIGHT,
        },
    })
}

/// Orange for the first half, then blue, so a dropped or reordered frame shows up in its colour.
fn two_colour_composition() -> Value {
    json!({
        "timeline": { "fps": 30, "duration_frames": FRAMES },
        "render_settings": { "width": WIDTH, "height": HEIGHT },
        "nodes": [
            background(ORANGE),
            {
                "id": 2,
                "type": "shape",
                "params": {
                    "width": WIDTH,
                    "height": HEIGHT,
                    "fill_paint": [BLUE[0], BLUE[1], BLUE[2], 255],
                    "opacity": format!("=tween(0, 1, {}, {})", FRAMES / 2 - 1, 1),
                },
            },
            { "id": 3, "type": "merge" },
            { "id": 4, "type": "media_output" },
        ],
        "connections": [
            { "from_node": 1, "from_port": "output", "to_node": 3, "to_port": "base" },
            { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "overlay" },
            { "from_node": 3, "from_port": "output", "to_node": 4, "to_port": "source" },
        ],
    })
}

fn video_composition(source: &str) -> Value {
    json!({
        "timeline": { "fps": 30, "duration_frames": FRAMES },
        "render_settings": { "width": WIDTH, "height": HEIGHT },
        "nodes": [
            { "id": 1, "type": "media_in", "params": { "kind": "video", "source": source } },
            { "id": 2, "type": "media_output" },
        ],
        "connections": [
            { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" },
        ],
    })
}

fn write_composition(directory: &Path, composition: &Value) -> PathBuf {
    let path = directory.join("composition.json");
    fs::write(&path, composition.to_string()).expect("write composition");
    path
}

fn write_solid_video(path: &Path, colour: [u8; 3]) {
    let config = VideoEncoderConfig::cpu_rgba(WIDTH, HEIGHT, 30, "libx264");
    let mut encoder =
        MuxedEncoder::create(path.to_string_lossy().to_string(), config).expect("create encoder");
    let pixels: Vec<u8> = colour
        .into_iter()
        .chain([255])
        .cycle()
        .take((WIDTH * HEIGHT * 4) as usize)
        .collect();
    for pts in 0..i64::from(FRAMES) {
        encoder
            .write_video_frame(&CpuVideoFrame {
                width: WIDTH,
                height: HEIGHT,
                stride: WIDTH as usize * 4,
                pixel_format: PixelFormat::Rgba8,
                pts: Some(pts),
                data: pixels.clone(),
            })
            .expect("write frame");
    }
    encoder.finish().expect("finish encode");
}

/// Decodes the first video stream of `path` to the YUV planes it stores, which sidesteps any colour
/// conversion.
fn decoded_frames(path: &Path) -> Vec<YuvVideoFrame> {
    let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open video");
    let stream_index = input
        .best_video_stream()
        .expect("video stream")
        .stream_index;
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .expect("open decoder");
    let mut frames = Vec::new();
    let mut take = |decoder: &mut VideoDecoder| {
        while let Some(frame) = decoder.receive_video_frame().expect("receive frame") {
            match frame {
                DecodedVideoFrame::Yuv(frame) => frames.push(frame),
                DecodedVideoFrame::Rgba { .. } => panic!("the encoder writes a YUV stream"),
            }
        }
    };
    while let Some(packet) = input.read_packet().expect("read packet") {
        decoder.send_packet(&packet).expect("send packet");
        take(&mut decoder);
    }
    decoder.send_eof().expect("send eof");
    take(&mut decoder);
    frames
}

/// The BT.709 limited-range Y'CbCr the encoder writes for an RGB colour.
fn encoded_ycbcr(rgb: [u8; 3]) -> [f64; 3] {
    const KR: f64 = 0.2126;
    const KB: f64 = 0.0722;
    let [r, g, b] = rgb.map(|channel| f64::from(channel) / 255.0);
    let y = KR * r + (1.0 - KR - KB) * g + KB * b;
    [
        16.0 + 219.0 * y,
        128.0 + 224.0 * (b - y) / (2.0 * (1.0 - KB)),
        128.0 + 224.0 * (r - y) / (2.0 * (1.0 - KR)),
    ]
}

/// Checks the frame count and that frame `n` shows `expected(n)` at its centre and corners.
fn assert_frames(path: &Path, expected: impl Fn(u32) -> [u8; 3]) {
    let frames = decoded_frames(path);
    assert_eq!(frames.len() as u32, FRAMES);
    for (frame, decoded) in frames.iter().enumerate() {
        assert_eq!((decoded.width, decoded.height), (WIDTH, HEIGHT));
        let want = encoded_ycbcr(expected(frame as u32));
        for (x, y) in [(WIDTH / 2, HEIGHT / 2), (4, 4), (WIDTH - 5, HEIGHT - 5)] {
            let sample = decoded.sample(x, y);
            for (got, channel) in sample.iter().zip(want) {
                assert!(
                    (f64::from(*got) - channel).abs() <= f64::from(MAX_SAMPLE_ERROR),
                    "frame {frame} at {x},{y}: got {sample:?}, want {want:?}",
                );
            }
        }
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn default_encoder_falls_back_to_libx264_with_one_warning_and_a_correct_video() {
    let Some(icd) = lavapipe_icd() else { return };
    let directory = tempfile::tempdir().expect("temp dir");
    let composition = write_composition(directory.path(), &two_colour_composition());
    let video = directory.path().join("out.mp4");

    let output = lumen(
        &icd,
        &[
            "render",
            composition.to_str().unwrap(),
            "--output",
            video.to_str().unwrap(),
        ],
        |_| {},
    );

    let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
    assert!(output.status.success(), "{stderr}");
    assert_eq!(
        stderr.matches("h264_vulkan is unavailable").count(),
        1,
        "{stderr}"
    );
    // An FFmpeg built without the encoder stops at the lookup, and any other at the device.
    assert!(
        stderr.contains("Vulkan device") || stderr.contains("requested encoder `h264_vulkan`"),
        "the warning should give the real reason: {stderr}"
    );
    assert!(
        stdout.contains("video_encoder name=libx264 path=cpu_upload"),
        "{stdout}"
    );
    assert_frames(
        &video,
        |frame| if frame < FRAMES / 2 { ORANGE } else { BLUE },
    );
}

#[test]
fn named_encoders_fail_instead_of_falling_back() {
    let Some(icd) = lavapipe_icd() else { return };
    let directory = tempfile::tempdir().expect("temp dir");
    let composition = write_composition(directory.path(), &two_colour_composition());
    let video = directory.path().join("out.mp4");
    let base_args = [
        "render",
        composition.to_str().unwrap(),
        "--output",
        video.to_str().unwrap(),
    ];

    let by_flag = lumen(
        &icd,
        &[&base_args[..], &["--encoder", "h264_vulkan"]].concat(),
        |_| {},
    );
    let by_env = lumen(&icd, &base_args, |command| {
        command.env("LUMEN_VIDEO_ENCODER", "h264_vulkan");
    });

    for output in [by_flag, by_env] {
        let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
        assert!(!output.status.success(), "{stdout}");
        assert!(stderr.contains("h264_vulkan"), "{stderr}");
        assert!(!stdout.contains("name=libx264"), "{stdout}");
        assert!(!video.exists());
    }
}

#[test]
fn hardware_decode_that_cannot_run_falls_back_to_the_cpu_with_one_warning() {
    let Some(icd) = lavapipe_icd() else { return };
    let directory = tempfile::tempdir().expect("temp dir");
    write_solid_video(&directory.path().join("clip.mp4"), ORANGE);
    let composition = write_composition(directory.path(), &video_composition("clip.mp4"));
    let render = |name: &str, hardware_decode: &str| {
        let video = directory.path().join(name);
        let output = lumen(
            &icd,
            &[
                "render",
                composition.to_str().unwrap(),
                "--media-root",
                directory.path().to_str().unwrap(),
                "--output",
                video.to_str().unwrap(),
            ],
            |command| {
                command.env("LUMEN_HARDWARE_DECODE", hardware_decode);
            },
        );
        let stderr = text(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        (video, stderr)
    };

    let (cpu_video, cpu_stderr) = render("cpu.mp4", "0");
    assert!(!cpu_stderr.contains("hardware decode"), "{cpu_stderr}");
    let (fallback_video, stderr) = render("fallback.mp4", "1");
    let warnings = stderr.matches("hardware decode failed").count()
        + stderr.matches("hardware decode unavailable").count();
    assert_eq!(warnings, 1, "{stderr}");

    let (cpu, fallback) = (decoded_frames(&cpu_video), decoded_frames(&fallback_video));
    for (frame, (cpu, fallback)) in cpu.iter().zip(&fallback).enumerate() {
        assert_eq!(cpu.data, fallback.data, "frame {frame}");
    }
}
