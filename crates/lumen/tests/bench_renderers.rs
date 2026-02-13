//! Quick renderer throughput comparison.
//!
//! Renders each benchmark fixture N times with both backends and prints timing.
//!
//! Run:
//!   cargo test -p lumen --features "renderer-vello renderer-skia" --test bench_renderers --release -- --nocapture

#![cfg(all(feature = "renderer-vello", feature = "renderer-skia"))]

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lumen::{
    backend::{NoopFrameProvider, RenderBackend},
    compile::compile_project,
    model::Project,
};

const WARMUP_FRAMES: u32 = 3;
const BENCH_FRAMES: u32 = 30;

fn require_release_profile() -> bool {
    if cfg!(debug_assertions) {
        eprintln!();
        eprintln!("skipping bench_renderers in debug profile");
        eprintln!("run with --release for meaningful renderer throughput numbers:");
        eprintln!(
            "  cargo test -p lumen --features \"renderer-vello renderer-skia\" --test bench_renderers --release -- --nocapture",
        );
        eprintln!();
        return false;
    }

    true
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs")
        .join("bench")
        .join("fixtures")
}

fn load_fixture(name: &str) -> Project {
    let path = fixtures_dir().join(name);
    let json = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("failed to parse fixture {}: {e}", path.display()))
}

fn bench_vello(project: &Project, warmup: u32, frames: u32) -> (Duration, Vec<Duration>) {
    let timeline = compile_project(project).expect("compile");
    let mut renderer =
        lumen::backend::vello::GpuRenderer::new(timeline.canvas.width, timeline.canvas.height)
            .expect("vello init");
    let mut provider = NoopFrameProvider;

    // Warmup
    for _ in 0..warmup {
        renderer
            .render_frame(&timeline, 0, &mut provider)
            .expect("vello render");
    }

    // Bench
    let mut frame_times = Vec::with_capacity(frames as usize);
    let start = Instant::now();
    for _ in 0..frames {
        let t = Instant::now();
        renderer
            .render_frame(&timeline, 0, &mut provider)
            .expect("vello render");
        frame_times.push(t.elapsed());
    }
    (start.elapsed(), frame_times)
}

fn bench_skia(project: &Project, warmup: u32, frames: u32) -> (Duration, Vec<Duration>) {
    let timeline = compile_project(project).expect("compile");
    let mut renderer =
        lumen::backend::skia::SkiaRenderer::new(timeline.canvas.width, timeline.canvas.height)
            .expect("skia init");
    let mut provider = NoopFrameProvider;

    // Warmup
    for _ in 0..warmup {
        renderer
            .render_frame(&timeline, 0, &mut provider)
            .expect("skia render");
    }

    // Bench
    let mut frame_times = Vec::with_capacity(frames as usize);
    let start = Instant::now();
    for _ in 0..frames {
        let t = Instant::now();
        renderer
            .render_frame(&timeline, 0, &mut provider)
            .expect("skia render");
        frame_times.push(t.elapsed());
    }
    (start.elapsed(), frame_times)
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() as f64) * p / 100.0).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn print_stats(_name: &str, backend: &str, total: Duration, frame_times: &mut Vec<Duration>) {
    frame_times.sort();
    let avg = total / frame_times.len() as u32;
    let p50 = percentile(frame_times, 50.0);
    let p95 = percentile(frame_times, 95.0);
    let p99 = percentile(frame_times, 99.0);
    let fps = frame_times.len() as f64 / total.as_secs_f64();

    eprintln!(
        "  {backend:>6}  avg={avg:>8.2?}  p50={p50:>8.2?}  p95={p95:>8.2?}  p99={p99:>8.2?}  {fps:>6.1} fps",
    );
}

#[test]
fn renderer_throughput_comparison() {
    if !require_release_profile() {
        return;
    }

    let fixtures = ["vector-heavy.json", "text-heavy.json", "mixed-media.json"];

    eprintln!();
    eprintln!(
        "=== Renderer Throughput Comparison ({BENCH_FRAMES} frames, {WARMUP_FRAMES} warmup) ==="
    );
    eprintln!();

    for fixture_name in &fixtures {
        let fixture_path = fixtures_dir().join(fixture_name);
        if !fixture_path.exists() {
            eprintln!("skipping {fixture_name}: not found");
            continue;
        }

        let project = load_fixture(fixture_name);
        eprintln!(
            "{fixture_name} ({}x{}):",
            project.canvas.width, project.canvas.height
        );

        let (vello_total, mut vello_times) = bench_vello(&project, WARMUP_FRAMES, BENCH_FRAMES);
        print_stats(fixture_name, "vello", vello_total, &mut vello_times);

        let (skia_total, mut skia_times) = bench_skia(&project, WARMUP_FRAMES, BENCH_FRAMES);
        print_stats(fixture_name, "skia", skia_total, &mut skia_times);

        let ratio = skia_total.as_secs_f64() / vello_total.as_secs_f64();
        if ratio < 1.0 {
            eprintln!("  => Skia is {:.1}x faster", 1.0 / ratio);
        } else {
            eprintln!("  => Vello is {:.1}x faster", ratio);
        }
        eprintln!();
    }
}
