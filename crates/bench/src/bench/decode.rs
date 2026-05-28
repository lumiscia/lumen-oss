use std::{
    fs,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow};
use lumen_ffmpeg::{
    CpuVideoFrame, DecodeMode, InputContext, MuxedEncoder, PixelFormat, VideoCodec, VideoDecoder,
    VideoDecoderConfig, VideoEncoderConfig,
};

#[derive(Debug)]
struct Args {
    frames: i64,
    save: Option<PathBuf>,
}

use crate::bench::{
    Bench,
    report::{SummaryReport, format_duration, format_fps},
};

pub struct DecodeBench;

impl Bench for DecodeBench {
    fn name() -> &'static str {
        "decode"
    }

    fn run() -> anyhow::Result<()> {
        run_inner()
    }
}

fn run_inner() -> anyhow::Result<()> {
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
    let crate_rss = max_rss_platform_units();

    // Warm up source media and codec pages so the timed CLI run measures decode work,
    // not cold file cache or one-off process startup.
    let _ = time_ffmpeg_cli_decode(&path)?;

    let (cli_startup, cli_decode) = time_ffmpeg_cli_decode(&path)?;
    let cli_rss = max_rss_platform_units();

    println!(
        "decode_bench frames={} crate_frames={} crate_decode_ms={} ffmpeg_cli_startup_ms={} ffmpeg_cli_decode_ms={} crate_fps={:.2} ffmpeg_cli_fps={:.2} crate_max_rss_platform_units={} after_cli_max_rss_platform_units={} source={}",
        args.frames,
        crate_frames,
        crate_elapsed.as_millis(),
        cli_startup.as_millis(),
        cli_decode.as_millis(),
        args.frames as f64 / crate_elapsed.as_secs_f64().max(1e-9),
        args.frames as f64 / cli_decode.as_secs_f64().max(1e-9),
        crate_rss,
        cli_rss,
        path.display()
    );

    let frames = args.frames.max(0) as u32;
    let mut summary = SummaryReport::new(
        "Decode benchmark summary",
        ["backend", "frames", "elapsed", "fps", "max_rss"],
    );
    summary.push_row(vec![
        "lumen-ffmpeg".to_string(),
        crate_frames.to_string(),
        format_duration(crate_elapsed),
        format_fps(frames, crate_elapsed),
        crate_rss.to_string(),
    ]);
    summary.push_row(vec![
        "ffmpeg CLI".to_string(),
        frames.to_string(),
        format_duration(cli_decode),
        format_fps(frames, cli_decode),
        cli_rss.to_string(),
    ]);
    if frames > 0 && !crate_elapsed.is_zero() && !cli_decode.is_zero() {
        let speedup = cli_decode.as_secs_f64() / crate_elapsed.as_secs_f64();
        summary.push_row(vec![
            "crate vs CLI".to_string(),
            "-".to_string(),
            format!("{speedup:.2}× faster than CLI"),
            format!("{speedup:.2}×").to_string(),
            "-".to_string(),
        ]);
    }
    summary.print();

    if args.save.is_none() {
        let _ = fs::remove_file(path);
    }
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
                    .parse::<i64>()
                    .context("--frames must be a positive integer")?;
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

fn max_rss_platform_units() -> i64 {
    unsafe {
        let mut usage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0 {
            usage.ru_maxrss
        } else {
            0
        }
    }
}

fn make_video(path: &Path, frames: i64) -> anyhow::Result<()> {
    let config = VideoEncoderConfig::cpu_rgba(128, 72, 30, VideoCodec::H264);
    let mut encoder = MuxedEncoder::create(path.to_string_lossy().to_string(), config)
        .context("create source encoder")?;
    for pts in 0..frames {
        encoder.write_video_frame(&frame(128, 72, pts))?;
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

fn time_ffmpeg_cli_decode(path: &Path) -> anyhow::Result<(Duration, Duration)> {
    let startup_start = Instant::now();
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
    let startup = startup_start.elapsed();

    let decode_start = Instant::now();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg stdout unavailable"))?;
    io::copy(&mut stdout, &mut io::sink()).context("drain ffmpeg decode output")?;
    let decode = decode_start.elapsed();

    let status = child.wait().context("wait for ffmpeg decode")?;
    if !status.success() {
        return Err(anyhow!("ffmpeg CLI decode failed"));
    }
    Ok((startup, decode))
}

fn decode_with_crate(path: &Path) -> anyhow::Result<usize> {
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
        while decoder.receive_cpu_frame()?.is_some() {
            frames += 1;
        }
    }
    decoder.send_eof()?;
    while decoder.receive_cpu_frame()?.is_some() {
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
