#![cfg(feature = "legacy-decode-tests")]

use std::{path::PathBuf, process::Command, sync::Arc};

use lumen::{
    Rational,
    compile::compile_project,
    model::{
        BaseStyle, Canvas, ClipContent, ClipItem, ClipStyle, Layer, LayerItem, Project, Source,
        SourceKind, SourceMedia, StyleValue, Timeline, TransformStyle,
    },
};
use lumen_server::video::{FfmpegRenderBackend, RenderBackendOptions};

fn make_testsrc_video(
    width: u32,
    height: u32,
    source_fps_num: u32,
    total_frames: u64,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let output_path = dir.path().join("source.mp4");

    let output = Command::new("ffmpeg")
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!(
            "testsrc=size={width}x{height}:rate={source_fps_num}"
        ))
        .arg("-frames:v")
        .arg(total_frames.to_string())
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg(&output_path)
        .output()
        .expect("spawn ffmpeg");

    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    (dir, output_path)
}

fn file_project(
    width: u32,
    height: u32,
    timeline_fps_num: u32,
    source_path: &str,
    total_frames: u64,
) -> Project {
    let mut clip_style = ClipStyle::default();
    clip_style.base = BaseStyle {
        transform: TransformStyle {
            x: StyleValue::Value(0.0),
            y: StyleValue::Value(0.0),
            width: StyleValue::Value(width as f32),
            height: StyleValue::Value(height as f32),
            ..Default::default()
        },
        ..Default::default()
    };

    Project {
        version: "1".to_string(),
        canvas: Canvas {
            width,
            height,
            background: [0, 0, 0, 255],
        },
        timeline: Timeline {
            fps: Rational::new(timeline_fps_num, 1),
            duration_frames: total_frames,
        },
        sources: vec![Source {
            id: "gen_video".to_string(),
            media: SourceMedia::Video,
            kind: SourceKind::File {
                path: source_path.to_string(),
            },
        }],
        layers: vec![Layer {
            id: "layer_0".to_string(),
            items: vec![LayerItem::Clip(ClipItem {
                id: "clip_0".to_string(),
                start_frame: 0,
                duration_frames: total_frames,
                content: ClipContent::Video {
                    source: "gen_video".to_string(),
                    pipeline: Default::default(),
                },
                style: clip_style,
                mask: None,
            })],
        }],
        audio: Default::default(),
    }
}

fn png_has_non_black_pixel(bytes: &[u8]) -> bool {
    let decoded = image::load_from_memory(bytes).expect("decode png");
    decoded
        .to_rgba8()
        .pixels()
        .any(|pixel| pixel.0[0] != 0 || pixel.0[1] != 0 || pixel.0[2] != 0)
}

#[test]
fn sequential_frames_do_not_fall_through_to_blank_when_source_fps_is_lower() {
    let (temp, source_path) = make_testsrc_video(640, 360, 24, 24);
    let project = file_project(640, 360, 30, source_path.to_str().expect("path"), 24);
    let timeline = compile_project(&project).expect("compile");
    let mut backend = FfmpegRenderBackend::new_with_options(
        Arc::clone(&timeline),
        RenderBackendOptions {
            media_root: Some(temp.path().to_path_buf()),
            ..Default::default()
        },
    );

    for frame in 0..timeline.total_frames() {
        let png = backend.render_frame_png(frame).expect("render");
        assert!(
            png_has_non_black_pixel(&png),
            "frame {frame} rendered a blank image"
        );
    }
}

#[test]
fn random_access_frames_stay_decodable_with_fps_mismatch() {
    let (temp, source_path) = make_testsrc_video(640, 360, 24, 24);
    let project = file_project(640, 360, 30, source_path.to_str().expect("path"), 24);
    let timeline = compile_project(&project).expect("compile");
    let mut backend = FfmpegRenderBackend::new_with_options(
        Arc::clone(&timeline),
        RenderBackendOptions {
            media_root: Some(temp.path().to_path_buf()),
            ..Default::default()
        },
    );

    let access_pattern = [0u64, 6, 12, 4, 8, 3, 10, 2, 14];
    for frame in access_pattern {
        let png = backend.render_frame_png(frame).expect("render");
        assert!(
            png_has_non_black_pixel(&png),
            "frame {frame} rendered a blank image"
        );
    }
}
