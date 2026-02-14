#![cfg(feature = "decode-libav")]

use std::sync::Arc;

use lumen::compile::compile_project;
use lumen::model::Project;
use lumen_server::video::FfmpegRenderBackend;

fn generator_project(
    width: u32,
    height: u32,
    timeline_fps_num: u32,
    source_fps_num: u32,
    total_frames: u64,
) -> Project {
    let json = serde_json::json!({
        "canvas": {
            "width": width,
            "height": height,
            "background": [0, 0, 0, 255]
        },
        "timeline": {
            "fps": { "num": timeline_fps_num, "den": 1 },
            "total_frames": total_frames
        },
        "sources": [{
            "id": "gen_video",
            "kind": "generator",
            "media": "video",
            "filter": format!("testsrc=size={width}x{height}:rate={source_fps_num}")
        }],
        "layers": [{
            "id": "layer_0",
            "z_index": 0,
            "items": [{
                "kind": "clip",
                "id": "clip_0",
                "start_frame": 0,
                "duration_frames": total_frames,
                "opacity": 1.0,
                "transform": {
                    "x": 0, "y": 0,
                    "width": width, "height": height,
                    "rotation_degrees": 0
                },
                "content": {
                    "type": "video",
                    "source": "gen_video",
                    "fit": "cover",
                    "pipeline": {
                        "speed": 1.0,
                        "reverse": false,
                        "looping": { "mode": "none" }
                    }
                }
            }]
        }],
        "audio": { "tracks": [] }
    });
    serde_json::from_value(json).expect("valid project JSON")
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
    let project = generator_project(640, 360, 30, 24, 24);
    let timeline = Arc::new(compile_project(&project).expect("compile"));
    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));

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
    let project = generator_project(640, 360, 30, 24, 24);
    let timeline = Arc::new(compile_project(&project).expect("compile"));
    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));

    let access_pattern = [0u64, 6, 12, 4, 8, 3, 10, 2, 14];
    for frame in access_pattern {
        let png = backend.render_frame_png(frame).expect("render");
        assert!(
            png_has_non_black_pixel(&png),
            "frame {frame} rendered a blank image"
        );
    }
}
