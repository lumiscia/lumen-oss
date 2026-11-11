#![cfg(target_os = "linux")]

//! Drives the real `lumen-local` binary on lavapipe to write stills, image sequences and ranged
//! videos. Skips when lavapipe isn't installed.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use lumen_ffmpeg::{DecodeMode, DecodedVideoFrame, InputContext, VideoDecoder, VideoDecoderConfig};
use serde_json::json;

fn lavapipe_icd() -> Option<PathBuf> {
    let icd = fs::read_dir("/usr/share/vulkan/icd.d")
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("lvp_icd"))
        });
    if icd.is_none() {
        eprintln!("lavapipe not installed; skipping lumen-local image tests");
    }
    icd
}

fn lumen_local(icd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lumen-local"))
        .args(args)
        .env_remove("LUMEN_VIDEO_ENCODER")
        .env_remove("LUMEN_HARDWARE_DECODE")
        .env("VK_ICD_FILENAMES", icd)
        .env("LUMEN_GPU_FORCE_FALLBACK_ADAPTER", "1")
        .env("WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER", "1")
        .env("WGPU_BACKEND", "vulkan")
        .env("WGPU_POWER_PREF", "none")
        .env("NO_COLOR", "1")
        .output()
        .expect("run lumen-local")
}

/// Red grows with the frame number, so a frame's image says which frame it is.
fn write_composition(directory: &Path) -> PathBuf {
    let path = directory.join("composition.json");
    let composition = json!({
        "timeline": { "fps": 30, "duration_frames": 30 },
        "render_settings": { "width": 64, "height": 48 },
        "nodes": [
            {
                "id": 1,
                "type": "background",
                "params": { "paint": "=tween(#000000, #ff0000, 0, 29, 'linear')", "width": 64, "height": 48 }
            },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    });
    fs::write(&path, composition.to_string()).expect("write composition");
    path
}

fn red_of(path: &Path) -> u8 {
    let image = image::open(path).expect("decode image").to_rgb8();
    assert_eq!(image.dimensions(), (64, 48));
    image.get_pixel(32, 24)[0]
}

fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(directory)
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

fn video_frames(path: &Path) -> usize {
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
    let mut count = 0;
    let mut take = |decoder: &mut VideoDecoder| {
        while let Some(frame) = decoder.receive_video_frame().expect("receive frame") {
            assert!(matches!(frame, DecodedVideoFrame::Yuv(_)));
            count += 1;
        }
    };
    while let Some(packet) = input.read_packet().expect("read packet") {
        decoder.send_packet(&packet).expect("send packet");
        take(&mut decoder);
    }
    decoder.send_eof().expect("send eof");
    take(&mut decoder);
    count
}

#[test]
fn writes_stills_sequences_and_ranged_videos() {
    let Some(icd) = lavapipe_icd() else { return };
    let directory = tempfile::tempdir().unwrap();
    let composition = write_composition(directory.path());
    let composition = composition.to_str().unwrap();
    let out = directory.path().join("out");
    let run = |args: &[&str]| {
        let output = lumen_local(&icd, &[&["--composition", composition], args].concat());
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&[
        "--output",
        out.join("early.png").to_str().unwrap(),
        "--frame",
        "3",
    ]);
    run(&[
        "--output",
        out.join("late.png").to_str().unwrap(),
        "--frame",
        "20",
    ]);
    run(&["--output", out.join("first.jpg").to_str().unwrap()]);
    let (early, late) = (
        red_of(&out.join("early.png")),
        red_of(&out.join("late.png")),
    );
    assert!(late > early + 80, "{early} then {late}");
    assert!(red_of(&out.join("first.jpg")) < 20);

    run(&[
        "--output",
        out.join("seq/shot_%d.png").to_str().unwrap(),
        "--frames",
        "5..9",
    ]);
    assert_eq!(
        names(&out.join("seq")),
        [
            "shot_0005.png",
            "shot_0006.png",
            "shot_0007.png",
            "shot_0008.png"
        ]
    );
    assert_eq!(red_of(&out.join("seq/shot_0008.png")), {
        run(&[
            "--output",
            out.join("eight.png").to_str().unwrap(),
            "--frame",
            "8",
        ]);
        red_of(&out.join("eight.png"))
    });
    run(&[
        "--output",
        out.join("jseq/%03d.jpg").to_str().unwrap(),
        "--frames",
        "..3",
        "--jpeg-quality",
        "60",
    ]);
    assert_eq!(names(&out.join("jseq")), ["000.jpg", "001.jpg", "002.jpg"]);

    run(&[
        "--output",
        out.join("clip.mp4").to_str().unwrap(),
        "--frames",
        "10..22",
    ]);
    assert_eq!(video_frames(&out.join("clip.mp4")), 12);
}

#[test]
fn refuses_ranges_the_timeline_does_not_have_without_writing_anything() {
    let Some(icd) = lavapipe_icd() else { return };
    let directory = tempfile::tempdir().unwrap();
    let composition = write_composition(directory.path());
    let composition = composition.to_str().unwrap();
    let out = directory.path().join("out");
    for (output, frames) in [
        ("a_%d.png", "20..31"),
        ("a.mp4", "5..5"),
        ("a.png", "30..31"),
    ] {
        let output = lumen_local(
            &icd,
            &[
                "--composition",
                composition,
                "--output",
                out.join(output).to_str().unwrap(),
                "--frames",
                frames,
            ],
        );
        assert!(!output.status.success(), "{frames}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("frame range"));
    }
    assert!(!out.exists() || names(&out).is_empty());
}
