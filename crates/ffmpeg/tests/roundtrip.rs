use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use lumen_ffmpeg::{
    AudioDecoder, AudioResampler, AudioResamplerConfig, CpuVideoFrame, DecodeMode, InputContext,
    MuxedEncoder, PixelFormat, VideoCodec, VideoDecoder, VideoDecoderConfig, VideoEncoderConfig,
};

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_ffmpeg_{name}_{unique}.{extension}"))
}

fn frame(width: u32, height: u32, pts: i64) -> CpuVideoFrame {
    let mut data = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 4;
            data[offset] = (x * 16) as u8;
            data[offset + 1] = (y * 16) as u8;
            data[offset + 2] = (pts * 20) as u8;
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

fn make_video(path: &PathBuf) -> bool {
    let config = VideoEncoderConfig {
        width: 32,
        height: 24,
        fps: 24,
        codec: VideoCodec::H264,
        ..VideoEncoderConfig::h264_rgba(32, 24, 24)
    };
    let Ok(mut encoder) = MuxedEncoder::create(path.to_string_lossy().to_string(), config) else {
        return false;
    };
    for pts in 0..6 {
        encoder
            .write_video_frame(&frame(32, 24, pts))
            .expect("write frame");
    }
    encoder.finish().expect("finish encode");
    true
}

#[test]
fn reports_missing_input_as_structured_error() {
    let error = InputContext::open("/definitely/not/a/media/file.mp4").expect_err("missing file");
    assert_eq!(error.operation, "avformat_open_input");
    assert_eq!(
        error.path.as_deref(),
        Some("/definitely/not/a/media/file.mp4")
    );
    assert!(error.code.is_some());
}

#[test]
fn encodes_opens_and_decodes_cpu_video() {
    let path = temp_path("video_roundtrip", "mp4");
    if !make_video(&path) {
        eprintln!("H.264 encoder unavailable; skipping video roundtrip");
        return;
    }

    let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let info = input.media_info();
    assert_eq!(info.video_streams.len(), 1);
    assert_eq!(info.video_streams[0].width, 32);
    assert_eq!(info.video_streams[0].height, 24);

    let stream = input.best_video_stream().expect("video stream");
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index: stream.stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .expect("decoder");

    let mut decoded = None;
    while decoded.is_none() {
        let Some(packet) = input.read_packet().expect("read packet") else {
            break;
        };
        decoder.send_packet(&packet).expect("send packet");
        decoded = decoder.receive_cpu_frame().expect("receive frame");
    }
    if decoded.is_none() {
        decoder.send_eof().expect("send eof");
        decoded = decoder.receive_cpu_frame().expect("flush frame");
    }

    let decoded = decoded.expect("decoded frame");
    assert_eq!(decoded.width, 32);
    assert_eq!(decoded.height, 24);
    assert_eq!(decoded.stride, 32 * 4);
    assert_eq!(decoded.data.len(), 32 * 24 * 4);

    let _ = fs::remove_file(path);
}

#[test]
fn decodes_and_resamples_audio_with_cli_fixture() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg CLI unavailable; skipping audio fixture test");
        return;
    }
    let path = temp_path("audio", "wav");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.1",
            "-ac",
            "1",
            path.to_str().expect("path"),
        ])
        .status()
        .expect("run ffmpeg");
    assert!(status.success());

    let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open audio");
    let stream = input.best_audio_stream().expect("audio stream");
    let mut decoder = AudioDecoder::open(&input, stream.stream_index).expect("audio decoder");

    let mut converted = None;
    while converted.is_none() {
        let Some(packet) = input.read_packet().expect("read packet") else {
            break;
        };
        decoder.send_packet(&packet).expect("send packet");
        if let Some(frame) = decoder.receive_frame().expect("receive audio") {
            let mut resampler =
                AudioResampler::new(&frame, AudioResamplerConfig::default()).expect("resampler");
            converted = Some(resampler.convert(&frame).expect("convert"));
        }
    }

    let converted = converted.expect("converted audio");
    assert_eq!(converted.sample_rate, 48_000);
    assert_eq!(converted.channels, 2);
    assert!(!converted.interleaved_f32.is_empty());

    let _ = fs::remove_file(path);
}
