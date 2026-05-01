use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use lumen_ffmpeg::{
    CpuVideoFrame, DecodeMode, InputContext, MuxedEncoder, PixelFormat, VideoCodec, VideoDecoder,
    VideoDecoderConfig, VideoEncoderConfig,
};

fn main() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg CLI not available; skipping benchmark");
        return;
    }

    let path = temp_path("mp4");
    if !make_video(&path, 120) {
        eprintln!("H.264 encoder unavailable; skipping benchmark");
        return;
    }

    let crate_start = Instant::now();
    let crate_frames = decode_with_crate(&path);
    let crate_elapsed = crate_start.elapsed();
    let crate_rss = max_rss_platform_units();

    let start = Instant::now();
    let status = Command::new("ffmpeg")
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
        .stdout(Stdio::null())
        .status()
        .expect("run ffmpeg decode");
    assert!(status.success());
    let cli_elapsed = start.elapsed();
    let cli_rss = max_rss_platform_units();

    println!("crate_decode_frames={crate_frames}");
    println!("crate_decode_ms={}", crate_elapsed.as_millis());
    println!("ffmpeg_cli_decode_ms={}", cli_elapsed.as_millis());
    println!("crate_max_rss_platform_units={crate_rss}");
    println!("after_cli_max_rss_platform_units={cli_rss}");

    let _ = fs::remove_file(path);
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

fn temp_path(extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_ffmpeg_bench_{unique}.{extension}"))
}

fn make_video(path: &PathBuf, frames: i64) -> bool {
    let config = VideoEncoderConfig {
        width: 128,
        height: 72,
        fps: 30,
        codec: VideoCodec::H264,
        ..VideoEncoderConfig::h264_rgba(128, 72, 30)
    };
    let Ok(mut encoder) = MuxedEncoder::create(path.to_string_lossy().to_string(), config) else {
        return false;
    };
    for pts in 0..frames {
        encoder
            .write_video_frame(&frame(128, 72, pts))
            .expect("write frame");
    }
    encoder.finish().expect("finish encode");
    true
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

fn decode_with_crate(path: &PathBuf) -> usize {
    let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open");
    let stream = input.best_video_stream().expect("video");
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index: stream.stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .expect("decoder");
    let mut frames = 0;
    while let Some(packet) = input.read_packet().expect("packet") {
        decoder.send_packet(&packet).expect("send");
        while decoder.receive_cpu_frame().expect("receive").is_some() {
            frames += 1;
        }
    }
    decoder.send_eof().expect("eof");
    while decoder.receive_cpu_frame().expect("flush").is_some() {
        frames += 1;
    }
    frames
}
