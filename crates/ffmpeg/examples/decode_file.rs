use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};

use lumen_ffmpeg::{DecodeMode, GpuBackend, InputContext, VideoDecoder, VideoDecoderConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: decode_file <path> [max_frames] [cpu|metal|vulkan]")?;
    let max_frames = args.next().and_then(|value| value.parse::<usize>().ok());
    let mode = match args.next().as_deref() {
        Some("metal") => DecodeMode::Gpu(GpuBackend::Metal),
        Some("vulkan") => DecodeMode::Gpu(GpuBackend::Vulkan),
        Some("cpu") | None => DecodeMode::Cpu,
        Some(other) => return Err(format!("unknown decode mode `{other}`").into()),
    };

    let started = Instant::now();
    let mut input = InputContext::open(path.to_string_lossy().to_string())?;
    let stream = input.best_video_stream()?;
    let open_elapsed = started.elapsed();
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index: stream.stream_index,
            mode,
        },
    )?;

    let decode_started = Instant::now();
    let mut frames = 0_usize;
    let mut bytes = 0_usize;

    'decode: while let Some(packet) = input.read_packet()? {
        decoder.send_packet(&packet)?;
        while let Some(frame_bytes) = receive_frame(&mut decoder, mode)? {
            frames = frames.saturating_add(1);
            bytes = bytes.saturating_add(frame_bytes);
            if max_frames.is_some_and(|limit| frames >= limit) {
                break 'decode;
            }
        }
    }

    if max_frames.is_none_or(|limit| frames < limit) {
        decoder.send_eof()?;
        while let Some(frame_bytes) = receive_frame(&mut decoder, mode)? {
            frames = frames.saturating_add(1);
            bytes = bytes.saturating_add(frame_bytes);
            if max_frames.is_some_and(|limit| frames >= limit) {
                break;
            }
        }
    }

    let decode_elapsed = decode_started.elapsed();
    let total_elapsed = started.elapsed();
    let usage = usage();
    println!("path={}", path.display());
    println!("codec={:?}", stream.codec);
    println!("dimensions={}x{}", stream.width, stream.height);
    println!("frames={frames}");
    println!("decoded_frame_bytes={bytes}");
    println!("open_ms={}", millis(open_elapsed));
    println!("decode_ms={}", millis(decode_elapsed));
    println!("total_ms={}", millis(total_elapsed));
    println!(
        "fps={:.2}",
        frames as f64 / decode_elapsed.as_secs_f64().max(1e-9)
    );
    println!("user_cpu_ms={}", millis(usage.user));
    println!("system_cpu_ms={}", millis(usage.system));
    println!("max_rss_platform_units={}", usage.max_rss);
    Ok(())
}

fn receive_frame(
    decoder: &mut VideoDecoder,
    mode: DecodeMode,
) -> lumen_ffmpeg::Result<Option<usize>> {
    match mode {
        DecodeMode::Cpu => Ok(decoder.receive_cpu_frame()?.map(|frame| frame.data.len())),
        DecodeMode::Gpu(_) => Ok(decoder.receive_gpu_frame()?.map(|_| 0)),
    }
}

fn millis(duration: Duration) -> u128 {
    duration.as_millis()
}

struct Usage {
    user: Duration,
    system: Duration,
    max_rss: i64,
}

fn usage() -> Usage {
    unsafe {
        let mut value = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut value) != 0 {
            return Usage {
                user: Duration::ZERO,
                system: Duration::ZERO,
                max_rss: 0,
            };
        }
        Usage {
            user: timeval_to_duration(value.ru_utime),
            system: timeval_to_duration(value.ru_stime),
            max_rss: value.ru_maxrss,
        }
    }
}

fn timeval_to_duration(value: libc::timeval) -> Duration {
    Duration::new(
        value.tv_sec.max(0) as u64,
        (value.tv_usec.max(0) as u32) * 1_000,
    )
}
