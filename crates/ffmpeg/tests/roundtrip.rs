use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use lumen_ffmpeg::{
    AudioDecoder, AudioEncoderConfig, AudioFrame, AudioResampler, AudioResamplerConfig,
    CpuVideoFrame, DecodeMode, InputContext, MuxedEncoder, PixelFormat, SampleFormat, VideoCodec,
    VideoDecoder, VideoDecoderConfig, VideoEncoderConfig,
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

fn audio_frame(sample_rate: u32, channels: u16, samples: usize, offset: usize) -> AudioFrame {
    let channels_usize = channels as usize;
    let mut interleaved_f32 = Vec::with_capacity(samples.saturating_mul(channels_usize));
    for sample in 0..samples {
        let t = (offset + sample) as f32 / sample_rate as f32;
        let value = (t * 440.0 * std::f32::consts::TAU).sin() * 0.2;
        for _ in 0..channels {
            interleaved_f32.push(value);
        }
    }
    AudioFrame {
        sample_rate,
        channels,
        sample_format: SampleFormat::F32,
        pts: Some(offset as i64),
        samples,
        interleaved_f32,
    }
}

fn make_video(path: &PathBuf) -> bool {
    let config = VideoEncoderConfig {
        width: 32,
        height: 24,
        fps: 24,
        codec: VideoCodec::H264,
        ..VideoEncoderConfig::cpu_rgba(32, 24, 24, VideoCodec::H264)
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

fn make_muxed_video(path: &PathBuf) -> bool {
    let video = VideoEncoderConfig::cpu_rgba(64, 36, 30, VideoCodec::H264);
    let audio = AudioEncoderConfig::aac(48_000, 2);
    let Ok(mut encoder) =
        MuxedEncoder::create_with_audio(path.to_string_lossy().to_string(), video, Some(audio))
    else {
        return false;
    };
    for pts in 0..30 {
        encoder
            .write_video_frame(&frame(64, 36, pts))
            .expect("write video frame");
    }
    let mut offset = 0;
    while offset < 48_000 {
        let samples = (48_000 - offset).min(1024);
        encoder
            .write_audio_frame(&audio_frame(48_000, 2, samples, offset))
            .expect("write audio frame");
        offset += samples;
    }
    encoder.finish().expect("finish muxed encode");
    true
}

fn make_video_with_encoder(path: &PathBuf, width: u32, height: u32, encoder_name: &str) -> bool {
    let mut config = VideoEncoderConfig::cpu_rgba(width, height, 30, VideoCodec::H264);
    config.encoder_name = Some(encoder_name.to_string());
    config.bit_rate = 2_000_000;
    let Ok(mut encoder) = MuxedEncoder::create(path.to_string_lossy().to_string(), config) else {
        return false;
    };
    for pts in 0..30 {
        encoder
            .write_video_frame(&frame(width, height, pts))
            .expect("write frame");
    }
    encoder.finish().expect("finish encode");
    true
}

#[cfg(all(feature = "cuda", target_os = "linux"))]
fn make_cuda_video_with_nvenc(path: &PathBuf, width: u32, height: u32) -> bool {
    use lumen_ffmpeg::{CudaDriver, EncodeMode, GpuBackend, GpuVideoInput};

    let driver = match CudaDriver::load() {
        Ok(driver) => driver,
        Err(error) => {
            eprintln!("CUDA driver unavailable; skipping CUDA NVENC test: {error}");
            return false;
        }
    };
    let _context = match driver.create_primary_context() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("CUDA context unavailable; skipping CUDA NVENC test: {error}");
            return false;
        }
    };
    let frame = match driver.allocate_rgba_frame(width, height) {
        Ok(frame) => frame,
        Err(error) => {
            eprintln!("CUDA frame allocation failed; skipping CUDA NVENC test: {error}");
            return false;
        }
    };
    if let Err(error) = _context.set_current() {
        eprintln!("CUDA context restore failed; skipping CUDA NVENC test: {error}");
        return false;
    }
    if let Err(error) = frame.clear(0x7f) {
        eprintln!("CUDA frame clear failed; skipping CUDA NVENC test: {error}");
        return false;
    }

    let mut config = VideoEncoderConfig::cpu_rgba(width, height, 30, VideoCodec::H264);
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    config.bit_rate = 2_000_000;
    let Ok(mut encoder) = MuxedEncoder::create(path.to_string_lossy().to_string(), config) else {
        return false;
    };
    for pts in 0..30 {
        let cuda_frame = frame.as_video_frame(Some(pts));
        let input = GpuVideoInput::Cuda(&cuda_frame);
        encoder.write_gpu_frame(&input).expect("write CUDA frame");
    }
    encoder.finish().expect("finish CUDA encode");
    true
}

#[cfg(all(feature = "cuda", target_os = "linux"))]
fn make_cuda_video_with_nvenc_and_audio(path: &PathBuf, width: u32, height: u32) -> bool {
    use lumen_ffmpeg::{CudaDriver, EncodeMode, GpuBackend, GpuVideoInput};

    let driver = match CudaDriver::load() {
        Ok(driver) => driver,
        Err(error) => {
            eprintln!("CUDA driver unavailable; skipping CUDA NVENC audio mux test: {error}");
            return false;
        }
    };
    let context = match driver.create_primary_context() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("CUDA context unavailable; skipping CUDA NVENC audio mux test: {error}");
            return false;
        }
    };
    let frame = match driver.allocate_rgba_frame(width, height) {
        Ok(frame) => frame,
        Err(error) => {
            eprintln!("CUDA frame allocation failed; skipping CUDA NVENC audio mux test: {error}");
            return false;
        }
    };
    if let Err(error) = context.set_current() {
        eprintln!("CUDA context restore failed; skipping CUDA NVENC audio mux test: {error}");
        return false;
    }
    if let Err(error) = frame.clear(0x7f) {
        eprintln!("CUDA frame clear failed; skipping CUDA NVENC audio mux test: {error}");
        return false;
    }

    let mut video = VideoEncoderConfig::cpu_rgba(width, height, 30, VideoCodec::H264);
    video.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    video.bit_rate = 2_000_000;
    let audio = AudioEncoderConfig::aac(48_000, 2);
    let Ok(mut encoder) =
        MuxedEncoder::create_with_audio(path.to_string_lossy().to_string(), video, Some(audio))
    else {
        return false;
    };
    for pts in 0..30 {
        let cuda_frame = frame.as_video_frame(Some(pts));
        let input = GpuVideoInput::Cuda(&cuda_frame);
        encoder.write_gpu_frame(&input).expect("write CUDA frame");
    }
    let mut offset = 0;
    while offset < 48_000 {
        let samples = (48_000 - offset).min(1024);
        encoder
            .write_audio_frame(&audio_frame(48_000, 2, samples, offset))
            .expect("write audio frame");
        offset += samples;
    }
    encoder.finish().expect("finish CUDA audio mux encode");
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
fn encodes_muxed_video_and_audio() {
    let path = temp_path("muxed_roundtrip", "mp4");
    if !make_muxed_video(&path) {
        eprintln!("H.264/AAC encoder unavailable; skipping muxed roundtrip");
        return;
    }

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let info = input.media_info();
    assert_eq!(info.video_streams.len(), 1);
    assert_eq!(info.audio_streams.len(), 1);
    assert_eq!(info.video_streams[0].codec, VideoCodec::H264);
    assert_eq!(info.video_streams[0].width, 64);
    assert_eq!(info.video_streams[0].height, 36);
    assert_eq!(info.audio_streams[0].sample_rate, 48_000);
    assert_eq!(info.audio_streams[0].channels, 2);

    let _ = fs::remove_file(path);
}

#[test]
fn encodes_cpu_frames_with_nvenc_when_requested() {
    if std::env::var_os("LUMEN_FFMPEG_TEST_NVENC").is_none() {
        eprintln!("set LUMEN_FFMPEG_TEST_NVENC=1 to run NVENC hardware smoke test");
        return;
    }

    let path = temp_path("nvenc_roundtrip", "mp4");
    if !make_video_with_encoder(&path, 640, 360, "h264_nvenc") {
        eprintln!("h264_nvenc unavailable; skipping NVENC video roundtrip");
        return;
    }

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let info = input.media_info();
    assert_eq!(info.video_streams.len(), 1);
    assert_eq!(info.video_streams[0].codec, VideoCodec::H264);
    assert_eq!(info.video_streams[0].width, 640);
    assert_eq!(info.video_streams[0].height, 360);

    let _ = fs::remove_file(path);
}

#[cfg(all(feature = "cuda", target_os = "linux"))]
#[test]
fn encodes_cuda_frames_with_nvenc_when_requested() {
    if std::env::var_os("LUMEN_FFMPEG_TEST_CUDA_NVENC").is_none() {
        eprintln!("set LUMEN_FFMPEG_TEST_CUDA_NVENC=1 to run CUDA frame NVENC smoke test");
        return;
    }

    let path = temp_path("cuda_nvenc_roundtrip", "mp4");
    if !make_cuda_video_with_nvenc(&path, 640, 360) {
        eprintln!("CUDA/NVENC unavailable; skipping CUDA frame NVENC roundtrip");
        return;
    }

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let info = input.media_info();
    assert_eq!(info.video_streams.len(), 1);
    assert_eq!(info.video_streams[0].codec, VideoCodec::H264);
    assert_eq!(info.video_streams[0].width, 640);
    assert_eq!(info.video_streams[0].height, 360);

    let _ = fs::remove_file(path);
}

#[cfg(all(feature = "cuda", target_os = "linux"))]
#[test]
fn encodes_cuda_frames_with_nvenc_and_audio_when_requested() {
    if std::env::var_os("LUMEN_FFMPEG_TEST_CUDA_NVENC_AUDIO").is_none() {
        eprintln!(
            "set LUMEN_FFMPEG_TEST_CUDA_NVENC_AUDIO=1 to run CUDA frame NVENC + AAC mux smoke test"
        );
        return;
    }

    let path = temp_path("cuda_nvenc_audio_roundtrip", "mp4");
    if !make_cuda_video_with_nvenc_and_audio(&path, 640, 360) {
        eprintln!("CUDA/NVENC/AAC unavailable; skipping CUDA frame NVENC audio mux roundtrip");
        return;
    }

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let info = input.media_info();
    assert_eq!(info.video_streams.len(), 1);
    assert_eq!(info.audio_streams.len(), 1);
    assert_eq!(info.video_streams[0].codec, VideoCodec::H264);
    assert_eq!(info.video_streams[0].width, 640);
    assert_eq!(info.video_streams[0].height, 360);
    assert_eq!(info.audio_streams[0].sample_rate, 48_000);
    assert_eq!(info.audio_streams[0].channels, 2);

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
