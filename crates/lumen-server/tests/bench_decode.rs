//! Video decode backend throughput benchmark.
//!
//! Measures frame decode + render performance for the currently enabled decode
//! backend (decode-libav or decode-subprocess).
//!
//! Run each backend separately and compare:
//!
//!   cargo test -p lumen-server --release --test bench_decode -- --nocapture
//!   cargo test -p lumen-server --release --test bench_decode --no-default-features \
//!       --features "renderer-vello decode-subprocess" -- --nocapture

use std::sync::Arc;
use std::time::{Duration, Instant};

use lumen::compile::compile_project;
use lumen::model::Project;
use lumen_server::video::FfmpegRenderBackend;

const WARMUP_FRAMES: usize = 2;

fn backend_label() -> &'static str {
    if cfg!(feature = "decode-libav") {
        "libav"
    } else {
        "subprocess"
    }
}

fn generator_project(width: u32, height: u32, fps_num: u32, total_frames: u64) -> Project {
    let json = serde_json::json!({
        "canvas": {
            "width": width,
            "height": height,
            "background": [0, 0, 0, 255]
        },
        "timeline": {
            "fps": { "num": fps_num, "den": 1 },
            "total_frames": total_frames
        },
        "sources": [{
            "id": "gen_video",
            "kind": "generator",
            "media": "video",
            "filter": format!("testsrc=size={width}x{height}:rate={fps_num}")
        }],
        "layers": [{
            "id": "layer_0",
            "z_index": 0,
            "clips": [{
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

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() as f64) * p / 100.0).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn print_stats(label: &str, total: Duration, times: &mut Vec<Duration>) {
    times.sort();
    let avg = total / times.len() as u32;
    let p50 = percentile(times, 50.0);
    let p95 = percentile(times, 95.0);
    let p99 = percentile(times, 99.0);
    let fps = times.len() as f64 / total.as_secs_f64();
    eprintln!(
        "  {label:<30} avg={avg:>8.2?}  p50={p50:>8.2?}  p95={p95:>8.2?}  p99={p99:>8.2?}  {fps:>6.1} fps",
    );
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

/// Sequential frame decode via render_frame_png (batch decode path).
/// Each call decodes a single frame from the generator source.
#[test]
fn sequential_frame_decode() {
    let project = generator_project(1280, 720, 30, 60);
    let timeline = Arc::new(compile_project(&project).expect("compile"));
    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));

    // Warmup
    for i in 0..WARMUP_FRAMES as u64 {
        backend.render_frame_png(i).expect("warmup");
    }

    let frames = 30u64;
    let mut times = Vec::with_capacity(frames as usize);
    let start = Instant::now();
    for frame in 0..frames {
        let t = Instant::now();
        backend.render_frame_png(frame).expect("render");
        times.push(t.elapsed());
    }
    let total = start.elapsed();

    eprintln!();
    eprintln!("=== Sequential Frame Decode — {backend} (1280x720 @ 30fps) ===", backend = backend_label());
    print_stats("render_frame_png (sequential)", total, &mut times);
    eprintln!("  total: {total:.2?} for {frames} frames");
    eprintln!();
}

/// Random-access frame decode — simulates seeking by requesting frames in a
/// non-sequential pattern (forward jumps and backward seeks).
#[test]
fn random_access_decode() {
    let project = generator_project(1280, 720, 30, 120);
    let timeline = Arc::new(compile_project(&project).expect("compile"));
    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));

    // Warmup
    backend.render_frame_png(0).expect("warmup");

    // Access pattern: forward skip, backward seek, random jumps
    let access_pattern: Vec<u64> = vec![
        0, 5, 10, 15, 20,   // forward sequential with gaps
        18, 12, 6,           // backward seeks
        50, 80, 110,         // large forward jumps
        30, 60, 90,          // backwards then forward
        119, 0, 60,          // extremes
    ];

    let mut times = Vec::with_capacity(access_pattern.len());
    let start = Instant::now();
    for &frame in &access_pattern {
        let t = Instant::now();
        backend.render_frame_png(frame).expect("render");
        times.push(t.elapsed());
    }
    let total = start.elapsed();

    eprintln!();
    eprintln!("=== Random Access Decode — {backend} (1280x720 @ 30fps) ===", backend = backend_label());
    print_stats("render_frame_png (random)", total, &mut times);
    eprintln!("  total: {total:.2?} for {} frames", access_pattern.len());
    eprintln!();
}

/// Full render_to_mp4 pipeline benchmark — streaming decode + render + encode.
#[test]
fn full_render_pipeline() {
    let project = generator_project(1280, 720, 30, 90);
    let timeline = Arc::new(compile_project(&project).expect("compile"));

    eprintln!();
    eprintln!("=== Full Render Pipeline — {backend} (1280x720, 90 frames @ 30fps) ===", backend = backend_label());

    // Warmup run
    {
        let warmup_project = generator_project(640, 360, 30, 5);
        let warmup_tl = Arc::new(compile_project(&warmup_project).expect("compile"));
        let mut warmup_backend = FfmpegRenderBackend::new(warmup_tl);
        warmup_backend.render_to_mp4(&mut |_, _| {}).expect("warmup render");
    }

    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));
    let mut frame_times = Vec::with_capacity(90);
    let start = Instant::now();
    let _mp4 = backend
        .render_to_mp4(&mut |_done, _total| {
            frame_times.push(Instant::now());
        })
        .expect("render_to_mp4");
    let total = start.elapsed();

    // Convert absolute timestamps to per-frame durations
    let mut durations: Vec<Duration> = Vec::with_capacity(frame_times.len());
    for i in 0..frame_times.len() {
        let prev = if i == 0 { start } else { frame_times[i - 1] };
        durations.push(frame_times[i].duration_since(prev));
    }

    print_stats("render_to_mp4 (per frame)", total, &mut durations);
    eprintln!("  total: {total:.2?}  output: {} bytes", _mp4.len());
    eprintln!();
}

/// 1080p sequential decode — measures scaling behavior at higher resolution.
#[test]
fn sequential_1080p() {
    let project = generator_project(1920, 1080, 30, 30);
    let timeline = Arc::new(compile_project(&project).expect("compile"));
    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));

    // Warmup
    backend.render_frame_png(0).expect("warmup");

    let frames = 15u64;
    let mut times = Vec::with_capacity(frames as usize);
    let start = Instant::now();
    for frame in 0..frames {
        let t = Instant::now();
        backend.render_frame_png(frame).expect("render");
        times.push(t.elapsed());
    }
    let total = start.elapsed();

    eprintln!();
    eprintln!("=== 1080p Sequential Decode — {backend} (1920x1080 @ 30fps) ===", backend = backend_label());
    print_stats("render_frame_png (1080p)", total, &mut times);
    eprintln!("  total: {total:.2?} for {frames} frames");
    eprintln!();
}
