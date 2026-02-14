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

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::{
    fs,
    process::{Command, Output, Stdio},
};

use lumen::compile::compile_project;
use lumen::model::Project;
use lumen_server::video::FfmpegRenderBackend;

const WARMUP_FRAMES: usize = 2;
static BENCH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bench_guard() -> std::sync::MutexGuard<'static, ()> {
    BENCH_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn require_release_profile() -> bool {
    if cfg!(debug_assertions) {
        eprintln!();
        eprintln!("skipping bench_decode in debug profile");
        eprintln!("run with --release for meaningful throughput numbers:");
        eprintln!("  cargo test -p lumen-server --release --test bench_decode -- --nocapture");
        eprintln!();
        return false;
    }

    true
}

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

fn print_total_rate(label: &str, total: Duration, units: usize, unit: &str) {
    let rate = units as f64 / total.as_secs_f64();
    eprintln!("  {label:<30} total={total:>8.2?}  {rate:>6.1} {unit}/s");
}

fn ffmpeg_decode_command(width: u32, height: u32, fps_num: u32, start: u64, end: u64) -> Command {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!("testsrc=size={width}x{height}:rate={fps_num}"))
        .arg("-an")
        .arg("-vf")
        .arg(format!(
            "fps={fps_num}/1,trim=start_frame={start}:end_frame={end},setpts=PTS-STARTPTS,format=rgba"
        ))
        .arg("-frames:v")
        .arg(end.saturating_sub(start).to_string())
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

fn ffmpeg_encode_command(
    width: u32,
    height: u32,
    fps_num: u32,
    frames: u64,
    output: &str,
) -> Command {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!("testsrc=size={width}x{height}:rate={fps_num}"))
        .arg("-an")
        .arg("-frames:v")
        .arg(frames.to_string())
        .arg("-c:v")
        .arg("libx264")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

fn ffmpeg_decode_filtered_command(
    width: u32,
    height: u32,
    fps_num: u32,
    filter_graph: &str,
    frames: u64,
) -> Command {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!("testsrc=size={width}x{height}:rate={fps_num}"))
        .arg("-an")
        .arg("-vf")
        .arg(filter_graph)
        .arg("-frames:v")
        .arg(frames.to_string())
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

fn ffmpeg_encode_filtered_command(
    width: u32,
    height: u32,
    fps_num: u32,
    filter_graph: &str,
    frames: u64,
    output: &str,
) -> Command {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!("testsrc=size={width}x{height}:rate={fps_num}"))
        .arg("-an")
        .arg("-vf")
        .arg(filter_graph)
        .arg("-frames:v")
        .arg(frames.to_string())
        .arg("-c:v")
        .arg("libx264")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

fn run_command(mut command: Command) -> Result<(Duration, Output), String> {
    let start = Instant::now();
    let output = command
        .output()
        .map_err(|err| format!("failed to spawn ffmpeg CLI command: {err}"))?;
    Ok((start.elapsed(), output))
}

fn ensure_success(output: &Output, context: &str) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "{context} failed with status {}: {}",
        output.status,
        stderr.trim()
    ))
}

fn bench_ffmpeg_sequential_decode(
    width: u32,
    height: u32,
    fps_num: u32,
    frames: u64,
) -> Result<Duration, String> {
    let warmup = ffmpeg_decode_command(width, height, fps_num, 0, 1);
    let (_, warmup_output) = run_command(warmup)?;
    ensure_success(&warmup_output, "ffmpeg decode warmup")?;

    let command = ffmpeg_decode_command(width, height, fps_num, 0, frames);
    let (elapsed, output) = run_command(command)?;
    ensure_success(&output, "ffmpeg decode benchmark")?;
    Ok(elapsed)
}

fn bench_ffmpeg_random_access_decode(
    width: u32,
    height: u32,
    fps_num: u32,
    access_pattern: &[u64],
) -> Result<(Duration, Vec<Duration>), String> {
    let warmup = ffmpeg_decode_command(width, height, fps_num, 0, 1);
    let (_, warmup_output) = run_command(warmup)?;
    ensure_success(&warmup_output, "ffmpeg random decode warmup")?;

    let mut times = Vec::with_capacity(access_pattern.len());
    let start = Instant::now();
    for &frame in access_pattern {
        let command = ffmpeg_decode_command(width, height, fps_num, frame, frame.saturating_add(1));
        let (elapsed, output) = run_command(command)?;
        ensure_success(&output, "ffmpeg random decode benchmark")?;
        times.push(elapsed);
    }
    Ok((start.elapsed(), times))
}

fn bench_ffmpeg_encode_baseline(
    width: u32,
    height: u32,
    fps_num: u32,
    frames: u64,
) -> Result<(Duration, u64), String> {
    let tempdir = tempfile::tempdir().map_err(|err| format!("failed to create tempdir: {err}"))?;
    let output_path = tempdir.path().join("ffmpeg-cli-baseline.mp4");
    let output_path_str = output_path
        .to_str()
        .ok_or_else(|| "failed to convert output path to UTF-8".to_string())?;

    let command = ffmpeg_encode_command(width, height, fps_num, frames, output_path_str);
    let (elapsed, output) = run_command(command)?;
    ensure_success(&output, "ffmpeg encode baseline")?;

    let size = fs::metadata(&output_path)
        .map_err(|err| format!("failed to stat ffmpeg output: {err}"))?
        .len();
    Ok((elapsed, size))
}

fn bench_ffmpeg_filtered_decode(
    width: u32,
    height: u32,
    fps_num: u32,
    filter_graph: &str,
    frames: u64,
    context: &str,
) -> Result<Duration, String> {
    let command = ffmpeg_decode_filtered_command(width, height, fps_num, filter_graph, frames);
    let (elapsed, output) = run_command(command)?;
    ensure_success(&output, context)?;
    Ok(elapsed)
}

fn bench_ffmpeg_filtered_encode(
    width: u32,
    height: u32,
    fps_num: u32,
    filter_graph: &str,
    frames: u64,
    context: &str,
) -> Result<(Duration, u64), String> {
    let tempdir = tempfile::tempdir().map_err(|err| format!("failed to create tempdir: {err}"))?;
    let output_path = tempdir.path().join("ffmpeg-cli-filtered.mp4");
    let output_path_str = output_path
        .to_str()
        .ok_or_else(|| "failed to convert output path to UTF-8".to_string())?;

    let command = ffmpeg_encode_filtered_command(
        width,
        height,
        fps_num,
        filter_graph,
        frames,
        output_path_str,
    );
    let (elapsed, output) = run_command(command)?;
    ensure_success(&output, context)?;

    let size = fs::metadata(&output_path)
        .map_err(|err| format!("failed to stat ffmpeg output: {err}"))?
        .len();
    Ok((elapsed, size))
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

/// Sequential decode-only benchmark.
#[test]
fn sequential_frame_decode() {
    if !require_release_profile() {
        return;
    }
    let _bench_guard = bench_guard();

    let project = generator_project(1280, 720, 30, 60);
    let timeline = Arc::new(compile_project(&project).expect("compile"));
    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));

    // Warmup
    for i in 0..WARMUP_FRAMES as u64 {
        backend.benchmark_decode_only_frame(i).expect("warmup");
    }

    let frames = 30u64;
    let mut times = Vec::with_capacity(frames as usize);
    let start = Instant::now();
    for frame in 0..frames {
        let t = Instant::now();
        backend.benchmark_decode_only_frame(frame).expect("decode");
        times.push(t.elapsed());
    }
    let total = start.elapsed();

    eprintln!();
    eprintln!(
        "=== Sequential Frame Decode — {backend} (1280x720 @ 30fps) ===",
        backend = backend_label()
    );
    print_stats("backend decode-only (sequential)", total, &mut times);
    match bench_ffmpeg_sequential_decode(1280, 720, 30, frames) {
        Ok(cli_total) => {
            print_total_rate(
                "ffmpeg CLI decode (sequential)",
                cli_total,
                frames as usize,
                "frames",
            );
        }
        Err(err) => {
            eprintln!("  ffmpeg CLI decode (sequential) skipped: {err}");
        }
    }
    eprintln!("  total: {total:.2?} for {frames} frames");
    eprintln!();
}

/// Random-access decode-only benchmark — simulates seeking by requesting frames
/// in a non-sequential pattern (forward jumps and backward seeks).
#[test]
fn random_access_decode() {
    if !require_release_profile() {
        return;
    }
    let _bench_guard = bench_guard();

    let project = generator_project(1280, 720, 30, 120);
    let timeline = Arc::new(compile_project(&project).expect("compile"));
    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));

    // Warmup
    backend.benchmark_decode_only_frame(0).expect("warmup");

    // Access pattern: forward skip, backward seek, random jumps
    let access_pattern: Vec<u64> = vec![
        0, 5, 10, 15, 20, // forward sequential with gaps
        18, 12, 6, // backward seeks
        50, 80, 110, // large forward jumps
        30, 60, 90, // backwards then forward
        119, 0, 60, // extremes
    ];

    let mut times = Vec::with_capacity(access_pattern.len());
    let start = Instant::now();
    for &frame in &access_pattern {
        let t = Instant::now();
        backend.benchmark_decode_only_frame(frame).expect("decode");
        times.push(t.elapsed());
    }
    let total = start.elapsed();

    eprintln!();
    eprintln!(
        "=== Random Access Decode — {backend} (1280x720 @ 30fps) ===",
        backend = backend_label()
    );
    print_stats("backend decode-only (random)", total, &mut times);
    match bench_ffmpeg_random_access_decode(1280, 720, 30, &access_pattern) {
        Ok((cli_total, mut cli_times)) => {
            print_stats("ffmpeg CLI decode (random)", cli_total, &mut cli_times);
            eprintln!(
                "  ffmpeg CLI random total: {cli_total:.2?} for {} frames",
                access_pattern.len()
            );
        }
        Err(err) => {
            eprintln!("  ffmpeg CLI decode (random) skipped: {err}");
        }
    }
    eprintln!("  total: {total:.2?} for {} frames", access_pattern.len());
    eprintln!();
}

/// Full render_to_mp4 pipeline benchmark — streaming decode + render + encode.
#[test]
fn full_render_pipeline() {
    if !require_release_profile() {
        return;
    }
    let _bench_guard = bench_guard();

    let project = generator_project(1280, 720, 30, 90);
    let timeline = Arc::new(compile_project(&project).expect("compile"));

    eprintln!();
    eprintln!(
        "=== Full Render Pipeline — {backend} (1280x720, 90 frames @ 30fps) ===",
        backend = backend_label()
    );

    // Warmup run
    {
        let warmup_project = generator_project(640, 360, 30, 5);
        let warmup_tl = Arc::new(compile_project(&warmup_project).expect("compile"));
        let mut warmup_backend = FfmpegRenderBackend::new(warmup_tl);
        warmup_backend
            .render_to_mp4(&mut |_, _| {})
            .expect("warmup render");
    }

    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));
    let mut progress_samples = Vec::with_capacity(90);
    let start = Instant::now();
    let _mp4 = backend
        .render_to_mp4(&mut |done, _total| {
            progress_samples.push((done, Instant::now()));
        })
        .expect("render_to_mp4");
    let total = start.elapsed();

    // Convert progress updates to per-frame durations. This remains accurate
    // even when a backend reports progress in large batches (e.g. fast-path).
    let mut durations: Vec<Duration> = Vec::with_capacity(90);
    let mut previous_done = 0u64;
    let mut previous_time = start;
    for (done, timestamp) in progress_samples {
        let step_frames = done.saturating_sub(previous_done);
        if step_frames == 0 {
            continue;
        }

        let step_duration = timestamp.duration_since(previous_time);
        let per_frame = step_duration / step_frames as u32;
        for _ in 0..step_frames {
            durations.push(per_frame);
        }

        previous_done = done;
        previous_time = timestamp;
    }

    if durations.is_empty() {
        durations.push(total);
    }

    print_stats("render_to_mp4 (per frame)", total, &mut durations);
    match bench_ffmpeg_encode_baseline(1280, 720, 30, 90) {
        Ok((cli_total, cli_bytes)) => {
            print_total_rate("ffmpeg CLI encode baseline", cli_total, 90, "frames");
            eprintln!("  ffmpeg CLI baseline output: {cli_bytes} bytes");
        }
        Err(err) => {
            eprintln!("  ffmpeg CLI encode baseline skipped: {err}");
        }
    }
    eprintln!("  total: {total:.2?}  output: {} bytes", _mp4.len());
    eprintln!();
}

/// 1080p sequential decode-only benchmark.
#[test]
fn sequential_1080p() {
    if !require_release_profile() {
        return;
    }
    let _bench_guard = bench_guard();

    let project = generator_project(1920, 1080, 30, 30);
    let timeline = Arc::new(compile_project(&project).expect("compile"));
    let mut backend = FfmpegRenderBackend::new(Arc::clone(&timeline));

    // Warmup
    backend.benchmark_decode_only_frame(0).expect("warmup");

    let frames = 15u64;
    let mut times = Vec::with_capacity(frames as usize);
    let start = Instant::now();
    for frame in 0..frames {
        let t = Instant::now();
        backend.benchmark_decode_only_frame(frame).expect("decode");
        times.push(t.elapsed());
    }
    let total = start.elapsed();

    eprintln!();
    eprintln!(
        "=== 1080p Sequential Decode — {backend} (1920x1080 @ 30fps) ===",
        backend = backend_label()
    );
    print_stats("backend decode-only (1080p)", total, &mut times);
    match bench_ffmpeg_sequential_decode(1920, 1080, 30, frames) {
        Ok(cli_total) => {
            print_total_rate(
                "ffmpeg CLI decode (1080p)",
                cli_total,
                frames as usize,
                "frames",
            );
        }
        Err(err) => {
            eprintln!("  ffmpeg CLI decode (1080p) skipped: {err}");
        }
    }
    eprintln!("  total: {total:.2?} for {frames} frames");
    eprintln!();
}

/// Direct ffmpeg CLI reverse/retime throughput baseline.
#[test]
fn ffmpeg_cli_retime_and_reverse() {
    if !require_release_profile() {
        return;
    }
    let _bench_guard = bench_guard();

    let width = 1280;
    let height = 720;
    let fps = 30;
    let output_frames = 90u64;

    // Use an input window larger than output to make retime/reverse realistic.
    let input_frames = 180u64;
    let retime_filter = format!(
        "fps={fps}/1,trim=start_frame=0:end_frame={input_frames},setpts=0.5*PTS,format=rgba"
    );
    let reverse_filter =
        format!("fps={fps}/1,trim=start_frame=0:end_frame={input_frames},reverse,format=rgba");
    let reverse_retime_filter = format!(
        "fps={fps}/1,trim=start_frame=0:end_frame={input_frames},reverse,setpts=0.5*PTS,format=rgba"
    );

    eprintln!();
    eprintln!("=== ffmpeg CLI Retiming/Reverse (1280x720 @ 30fps, 90 output frames) ===");

    match bench_ffmpeg_filtered_decode(
        width,
        height,
        fps,
        &retime_filter,
        output_frames,
        "ffmpeg decode retime 2x",
    ) {
        Ok(total) => print_total_rate(
            "ffmpeg CLI decode (retime 2x)",
            total,
            output_frames as usize,
            "frames",
        ),
        Err(err) => eprintln!("  ffmpeg CLI decode (retime 2x) skipped: {err}"),
    }

    match bench_ffmpeg_filtered_decode(
        width,
        height,
        fps,
        &reverse_filter,
        output_frames,
        "ffmpeg decode reverse",
    ) {
        Ok(total) => print_total_rate(
            "ffmpeg CLI decode (reverse)",
            total,
            output_frames as usize,
            "frames",
        ),
        Err(err) => eprintln!("  ffmpeg CLI decode (reverse) skipped: {err}"),
    }

    match bench_ffmpeg_filtered_decode(
        width,
        height,
        fps,
        &reverse_retime_filter,
        output_frames,
        "ffmpeg decode reverse+retime",
    ) {
        Ok(total) => print_total_rate(
            "ffmpeg CLI decode (reverse+2x)",
            total,
            output_frames as usize,
            "frames",
        ),
        Err(err) => eprintln!("  ffmpeg CLI decode (reverse+2x) skipped: {err}"),
    }

    match bench_ffmpeg_filtered_encode(
        width,
        height,
        fps,
        &retime_filter,
        output_frames,
        "ffmpeg encode retime 2x",
    ) {
        Ok((total, bytes)) => {
            print_total_rate(
                "ffmpeg CLI encode (retime 2x)",
                total,
                output_frames as usize,
                "frames",
            );
            eprintln!("  ffmpeg CLI encode (retime 2x) output: {bytes} bytes");
        }
        Err(err) => eprintln!("  ffmpeg CLI encode (retime 2x) skipped: {err}"),
    }

    match bench_ffmpeg_filtered_encode(
        width,
        height,
        fps,
        &reverse_filter,
        output_frames,
        "ffmpeg encode reverse",
    ) {
        Ok((total, bytes)) => {
            print_total_rate(
                "ffmpeg CLI encode (reverse)",
                total,
                output_frames as usize,
                "frames",
            );
            eprintln!("  ffmpeg CLI encode (reverse) output: {bytes} bytes");
        }
        Err(err) => eprintln!("  ffmpeg CLI encode (reverse) skipped: {err}"),
    }

    eprintln!();
}
