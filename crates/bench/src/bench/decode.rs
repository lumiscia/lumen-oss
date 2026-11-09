use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow, ensure};
use lumen_ffmpeg::{
    CpuVideoFrame, DecodeMode, InputContext, MuxedEncoder, PixelFormat, RateControl, VideoDecoder,
    VideoDecoderConfig, VideoEncoderConfig,
};

use crate::bench::report::{SummaryReport, format_duration, format_fps};

const WIDTH: u32 = 128;
const HEIGHT: u32 = 72;
const FRAME_BYTES: u64 = WIDTH as u64 * HEIGHT as u64 * 4;

#[derive(Debug)]
struct Args {
    frames: u32,
    save: Option<PathBuf>,
}

pub fn run() -> anyhow::Result<()> {
    let args = parse_args()?;
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return Err(anyhow!("ffmpeg CLI is unavailable"));
    }

    let path = args
        .save
        .clone()
        .unwrap_or_else(|| temp_path("source", "mp4"));
    make_video(&path, args.frames)?;

    let crate_start = Instant::now();
    let crate_frames = decode_with_crate(&path)?;
    let crate_elapsed = crate_start.elapsed();

    // Warm up source media and codec pages so the timed CLI run is not penalised for a cold
    // file cache.
    time_ffmpeg_cli_decode(&path)?;
    let (cli_bytes, cli_elapsed) = time_ffmpeg_cli_decode(&path)?;

    if args.save.is_none() {
        let _ = fs::remove_file(&path);
    }

    ensure!(
        crate_frames == args.frames,
        "lumen-ffmpeg decoded {crate_frames} frames, expected {}",
        args.frames
    );
    ensure!(
        cli_bytes == u64::from(args.frames) * FRAME_BYTES,
        "ffmpeg CLI wrote {cli_bytes} bytes, expected {} for {} RGBA frames",
        u64::from(args.frames) * FRAME_BYTES,
        args.frames
    );

    println!(
        "decode_bench frames={} crate_decode_ms={} ffmpeg_cli_total_ms={} crate_fps={:.2} ffmpeg_cli_fps={:.2} source={}",
        args.frames,
        crate_elapsed.as_millis(),
        cli_elapsed.as_millis(),
        f64::from(args.frames) / crate_elapsed.as_secs_f64().max(1e-9),
        f64::from(args.frames) / cli_elapsed.as_secs_f64().max(1e-9),
        path.display()
    );

    let mut summary = SummaryReport::new(
        "Decode benchmark summary",
        ["backend", "frames", "elapsed", "fps"],
    );
    summary.push_row(vec![
        "lumen-ffmpeg (in process)".to_string(),
        crate_frames.to_string(),
        format_duration(crate_elapsed),
        format_fps(args.frames, crate_elapsed),
    ]);
    summary.push_row(vec![
        "ffmpeg CLI (process + rawvideo pipe)".to_string(),
        args.frames.to_string(),
        format_duration(cli_elapsed),
        format_fps(args.frames, cli_elapsed),
    ]);
    summary.print();
    println!(
        "The CLI row is wall clock for the whole process, including start-up and piping RGBA frames; the crate row decodes in process and hands frames over without a pipe or an RGBA conversion, since YUV sources come out as planes. It is end-to-end, not decode-only throughput."
    );
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut frames = 120;
    let mut save = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => {
                frames = args
                    .next()
                    .ok_or_else(|| anyhow!("--frames requires a value"))?
                    .parse::<u32>()
                    .ok()
                    .filter(|&count| count > 0)
                    .ok_or_else(|| anyhow!("--frames must be a positive integer"))?;
            }
            "--save" => {
                save = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--save requires a path"))?,
                ));
            }
            "--help" | "-h" => {
                println!("usage: lumen-bench-decode [--frames N] [--save PATH]");
                std::process::exit(0);
            }
            _ => return Err(anyhow!("unknown argument `{arg}`")),
        }
    }
    Ok(Args { frames, save })
}

fn make_video(path: &Path, frames: u32) -> anyhow::Result<()> {
    let mut config = VideoEncoderConfig::cpu_rgba(WIDTH, HEIGHT, 30, "libx264");
    config.quality.rate_control = Some(RateControl::Bitrate(8_000_000));
    let mut encoder = MuxedEncoder::create(path.to_string_lossy().to_string(), config)
        .context("create source encoder")?;
    for pts in 0..i64::from(frames) {
        encoder.write_video_frame(&frame(WIDTH, HEIGHT, pts))?;
    }
    encoder.finish().context("finish source encode")
}

fn frame(width: u32, height: u32, pts: i64) -> CpuVideoFrame {
    let mut data = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 4;
            data[offset] = (x.wrapping_add(pts as u32) % 255) as u8;
            data[offset + 1] = (y.wrapping_mul(2) % 255) as u8;
            data[offset + 2] = (pts % 255) as u8;
            data[offset + 3] = 255;
        }
    }
    CpuVideoFrame {
        width,
        height,
        stride: width as usize * 4,
        pixel_format: PixelFormat::Rgba8,
        pts: Some(pts),
        data,
    }
}

/// Runs the ffmpeg CLI to completion and returns the RGBA bytes it wrote and its wall-clock time.
fn time_ffmpeg_cli_decode(path: &Path) -> anyhow::Result<(u64, Duration)> {
    let started = Instant::now();
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            path.to_str().expect("path"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn ffmpeg decode")?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg stdout unavailable"))?;
    let bytes = io::copy(&mut stdout, &mut io::sink()).context("drain ffmpeg decode output")?;
    let status = child.wait().context("wait for ffmpeg decode")?;
    let elapsed = started.elapsed();
    if !status.success() {
        return Err(anyhow!("ffmpeg CLI decode failed"));
    }
    Ok((bytes, elapsed))
}

fn decode_with_crate(path: &Path) -> anyhow::Result<u32> {
    let mut input = InputContext::open(path.to_string_lossy().to_string())?;
    let stream = input.best_video_stream()?;
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index: stream.stream_index,
            mode: DecodeMode::Cpu,
        },
    )?;
    let mut frames = 0;
    while let Some(packet) = input.read_packet()? {
        decoder.send_packet(&packet)?;
        while decoder.receive_video_frame()?.is_some() {
            frames += 1;
        }
    }
    decoder.send_eof()?;
    while decoder.receive_video_frame()?.is_some() {
        frames += 1;
    }
    Ok(frames)
}

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_bench_{name}_{unique}.{extension}"))
}
