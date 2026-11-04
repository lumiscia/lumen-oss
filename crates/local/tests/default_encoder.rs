#![cfg(target_os = "linux")]

//! Drives the real `lumen-local` binary on lavapipe, which has no Vulkan Video, so the Linux default
//! encoder and hardware decode both have to fall back. Skips like the engine's GPU tests when
//! lavapipe isn't installed.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::Arc,
};

use lumen_engine::{
    ffmpeg::FfmpegVideoResolver,
    media::{CpuMediaFrame, MediaFrame, VideoFrameResolver},
};
use lumen_ffmpeg::{CpuVideoFrame, MuxedEncoder, PixelFormat, VideoEncoderConfig};
use serde_json::{Value, json};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const FRAMES: u32 = 30;
const ORANGE: [u8; 3] = [255, 128, 32];
const BLUE: [u8; 3] = [32, 128, 255];
const MAX_CHANNEL_ERROR: i32 = 3;

fn lavapipe_icd() -> Option<PathBuf> {
    let icd = fs::read_dir("/usr/share/vulkan/icd.d")
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("lvp_icd"))
        });
    if icd.is_none() {
        eprintln!("lavapipe not installed; skipping lumen-local fallback test");
    }
    icd
}

fn lumen_local(icd: &Path, args: &[&str], configure: impl FnOnce(&mut Command)) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lumen-local"));
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
    command.output().expect("run lumen-local")
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

fn decoded_frames(path: &Path) -> Vec<Arc<CpuMediaFrame>> {
    let video = FfmpegVideoResolver::open(path.to_string_lossy().to_string()).expect("open output");
    let metadata = video.metadata();
    assert_eq!(
        (metadata.width, metadata.height, metadata.frame_count),
        (WIDTH, HEIGHT, FRAMES)
    );
    (0..FRAMES)
        .map(|frame| match video.frame(frame).expect("decode frame") {
            MediaFrame::CpuRgba(decoded) => decoded,
            _ => panic!("expected a CPU frame"),
        })
        .collect()
}

/// Checks the frame count and that frame `n` shows `expected(n)` at its centre and corners.
fn assert_frames(path: &Path, expected: impl Fn(u32) -> [u8; 3]) {
    for (frame, decoded) in decoded_frames(path).iter().enumerate() {
        let want = expected(frame as u32);
        for (x, y) in [(WIDTH / 2, HEIGHT / 2), (4, 4), (WIDTH - 5, HEIGHT - 5)] {
            let offset = decoded.row_bytes * y as usize + x as usize * 4;
            let pixel = &decoded.rgba[offset..offset + 3];
            for (channel, want) in pixel.iter().zip(want) {
                assert!(
                    (i32::from(*channel) - i32::from(want)).abs() <= MAX_CHANNEL_ERROR,
                    "frame {frame} at {x},{y}: got {pixel:?}, want {want:?}",
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

    let output = lumen_local(
        &icd,
        &[
            "--composition",
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
        "--composition",
        composition.to_str().unwrap(),
        "--output",
        video.to_str().unwrap(),
    ];

    let by_flag = lumen_local(
        &icd,
        &[&base_args[..], &["--encoder", "h264_vulkan"]].concat(),
        |_| {},
    );
    let by_env = lumen_local(&icd, &base_args, |command| {
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
        let output = lumen_local(
            &icd,
            &[
                "--composition",
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
        assert_eq!(cpu.rgba, fallback.rgba, "frame {frame}");
    }
}
