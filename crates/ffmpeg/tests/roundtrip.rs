use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use lumen_ffmpeg::{
    AudioDecoder, AudioEncoderConfig, AudioFrame, AudioResampler, AudioResamplerConfig,
    CpuVideoFrame, DecodeMode, InputContext, MuxedEncoder, PixelFormat, Rational, SampleFormat,
    VideoCodec, VideoDecoder, VideoDecoderConfig, VideoEncoderConfig,
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

fn make_video(path: &Path) -> bool {
    let config = VideoEncoderConfig {
        width: 32,
        height: 24,
        fps: Rational::from(24),
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

fn make_muxed_video(path: &Path) -> bool {
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

fn make_video_with_encoder(path: &Path, width: u32, height: u32, encoder_name: &str) -> bool {
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
    let context = match driver.create_primary_context() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("CUDA context unavailable; skipping CUDA NVENC test: {error}");
            return false;
        }
    };
    let frame = match context.allocate_rgba_frame(width, height) {
        Ok(frame) => frame,
        Err(error) => {
            eprintln!("CUDA frame allocation failed; skipping CUDA NVENC test: {error}");
            return false;
        }
    };
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
    let frame = match context.allocate_rgba_frame(width, height) {
        Ok(frame) => frame,
        Err(error) => {
            eprintln!("CUDA frame allocation failed; skipping CUDA NVENC audio mux test: {error}");
            return false;
        }
    };
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

fn assert_bt709_tags(path: &Path) {
    let Ok(output) = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("stream=color_space,color_transfer,color_primaries,color_range")
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .output()
    else {
        eprintln!("ffprobe unavailable; skipping colour tag assertions");
        return;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "color_space=bt709",
        "color_transfer=bt709",
        "color_primaries=bt709",
        "color_range=tv",
    ] {
        assert!(text.contains(expected), "missing {expected} in {text}");
    }
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

    assert_bt709_tags(&path);
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

#[test]
fn decodes_rgba_frames_across_resolution_change() {
    let clips = [("32x24", 32usize, 24usize), ("64x48", 64, 48)];
    let mut stream = Vec::new();
    for (size, _, _) in clips {
        let path = temp_path("resolution_clip", "h264");
        let available = run_fixture(
            Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
                .args(["-i", &format!("testsrc=size={size}:rate=10:duration=0.5")])
                .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "h264"])
                .arg(&path),
            "resolution change test",
        );
        if !available {
            return;
        }
        stream.extend(fs::read(&path).expect("read clip"));
        let _ = fs::remove_file(path);
    }
    let path = temp_path("resolution_change", "h264");
    fs::write(&path, stream).expect("write stream");

    let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open stream");
    let stream_index = input
        .best_video_stream()
        .expect("video stream")
        .stream_index;
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .expect("decoder");

    let mut sizes = Vec::new();
    let mut drain = |decoder: &mut VideoDecoder| {
        while let Some(frame) = decoder.receive_cpu_frame().expect("receive frame") {
            assert_eq!(frame.stride, frame.width as usize * 4);
            assert_eq!(frame.data.len(), frame.stride * frame.height as usize);
            assert!(frame.data.chunks_exact(4).all(|pixel| pixel[3] == 255));
            sizes.push((frame.width as usize, frame.height as usize));
        }
    };
    while let Some(packet) = input.read_packet().expect("read packet") {
        decoder.send_packet(&packet).expect("send packet");
        drain(&mut decoder);
    }
    decoder.send_eof().expect("send eof");
    drain(&mut decoder);

    assert!(sizes.contains(&(clips[0].1, clips[0].2)));
    assert!(sizes.contains(&(clips[1].1, clips[1].2)));
    let _ = fs::remove_file(path);
}

#[test]
fn encodes_fractional_frame_rate_exactly() {
    let path = temp_path("ntsc_roundtrip", "mp4");
    let config =
        VideoEncoderConfig::cpu_rgba(32, 24, Rational::new(30_000, 1001), VideoCodec::H264);
    let Ok(mut encoder) = MuxedEncoder::create(path.to_string_lossy().to_string(), config) else {
        eprintln!("H.264 encoder unavailable; skipping fractional frame rate test");
        return;
    };
    for pts in 0..30 {
        encoder
            .write_video_frame(&frame(32, 24, pts))
            .expect("write frame");
    }
    encoder.finish().expect("finish encode");

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let rate = input.media_info().video_streams[0].avg_frame_rate;
    assert_eq!(rate.numerator * 1001, rate.denominator * 30_000);

    let _ = fs::remove_file(path);
}

#[derive(Clone, Copy)]
struct Matrix {
    kr: f64,
    kb: f64,
}

// Luma weights published in ITU-R BT.709, BT.601 and BT.2020.
const BT709: Matrix = Matrix {
    kr: 0.2126,
    kb: 0.0722,
};
const BT601: Matrix = Matrix {
    kr: 0.299,
    kb: 0.114,
};
const BT2020: Matrix = Matrix {
    kr: 0.2627,
    kb: 0.0593,
};

const PATCHES: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [200, 120, 40]];

/// Converts full-range RGB to 8-bit Y'CbCr with the textbook equations, independently of swscale.
fn rgb_to_ycbcr(rgb: [u8; 3], matrix: Matrix, full_range: bool) -> [u8; 3] {
    let [r, g, b] = rgb.map(|channel| f64::from(channel) / 255.0);
    let kg = 1.0 - matrix.kr - matrix.kb;
    let y = matrix.kr * r + kg * g + matrix.kb * b;
    let cb = (b - y) / (2.0 * (1.0 - matrix.kb));
    let cr = (r - y) / (2.0 * (1.0 - matrix.kr));
    let (luma, chroma) = if full_range {
        (255.0 * y, 255.0)
    } else {
        (16.0 + 219.0 * y, 224.0)
    };
    [
        luma.round() as u8,
        (128.0 + chroma * cb).round().min(255.0) as u8,
        (128.0 + chroma * cr).round().min(255.0) as u8,
    ]
}

/// Writes raw YUV420P frames of equal-width vertical colour patches.
fn patch_frames(width: usize, height: usize, matrix: Matrix, full_range: bool) -> Vec<u8> {
    let colours = PATCHES.map(|rgb| rgb_to_ycbcr(rgb, matrix, full_range));
    let patch_at = |x: usize, plane_width: usize| colours[x * PATCHES.len() / plane_width];
    let mut frame = Vec::with_capacity(width * height * 3 / 2);
    for _ in 0..height {
        frame.extend((0..width).map(|x| patch_at(x, width)[0]));
    }
    for plane in [1, 2] {
        for _ in 0..height / 2 {
            frame.extend((0..width / 2).map(|x| patch_at(x, width / 2)[plane]));
        }
    }
    frame.repeat(2)
}

/// Runs an ffmpeg fixture command. Returns false, with the reason printed, only when the CLI isn't
/// installed or lacks a requested encoder; any other failure panics so a broken fixture can't pass.
fn run_fixture(command: &mut Command, name: &str) -> bool {
    let output = match command.output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("ffmpeg CLI not installed; skipping {name}");
            return false;
        }
        Err(error) => panic!("{name}: failed to run ffmpeg: {error}"),
    };
    if output.status.success() {
        return true;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("Unknown encoder") || stderr.contains("Encoder not found") {
        eprintln!(
            "ffmpeg encoder unavailable; skipping {name}: {}",
            stderr.trim()
        );
        return false;
    }
    panic!(
        "{name}: ffmpeg fixture failed ({}): {stderr}",
        output.status
    );
}

/// Encodes known Y'CbCr values with libx264, passing them through unconverted under the given tags.
fn encode_patches(
    name: &str,
    (width, height): (usize, usize),
    matrix: Matrix,
    full_range: bool,
    tags: &[&str],
) -> Option<PathBuf> {
    let raw = temp_path(name, "yuv");
    let path = temp_path(name, "h264");
    fs::write(&raw, patch_frames(width, height, matrix, full_range)).expect("write raw frames");
    let available = run_fixture(
        Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "rawvideo"])
            .args(["-pixel_format", "yuv420p", "-framerate", "30"])
            .args(["-video_size", &format!("{width}x{height}")])
            .args(tags)
            .arg("-i")
            .arg(&raw)
            .args(["-c:v", "libx264", "-preset", "ultrafast", "-qp", "1"])
            .args(tags)
            .args(["-f", "h264"])
            .arg(&path),
        name,
    );
    let _ = fs::remove_file(raw);
    if !available {
        return None;
    }
    Some(path)
}

fn decode_all_rgba(path: &Path) -> Vec<CpuVideoFrame> {
    let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open video");
    let stream_index = input
        .best_video_stream()
        .expect("video stream")
        .stream_index;
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .expect("decoder");
    let mut frames = Vec::new();
    while let Some(packet) = input.read_packet().expect("read packet") {
        decoder.send_packet(&packet).expect("send packet");
        while let Some(frame) = decoder.receive_cpu_frame().expect("receive frame") {
            frames.push(frame);
        }
    }
    decoder.send_eof().expect("send eof");
    while let Some(frame) = decoder.receive_cpu_frame().expect("receive frame") {
        frames.push(frame);
    }
    frames
}

fn pixel(frame: &CpuVideoFrame, x: usize, y: usize) -> [u8; 3] {
    let offset = y * frame.stride + x * 4;
    [
        frame.data[offset],
        frame.data[offset + 1],
        frame.data[offset + 2],
    ]
}

fn assert_close(actual: [u8; 3], expected: [u8; 3], context: &str) {
    const TOLERANCE: i32 = 3;
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (i32::from(*a) - i32::from(e)).abs() <= TOLERANCE),
        "{context}: decoded {actual:?}, expected {expected:?}"
    );
}

fn assert_patches(frame: &CpuVideoFrame, context: &str) {
    let width = frame.width as usize;
    for (index, expected) in PATCHES.iter().enumerate() {
        let x = (2 * index + 1) * width / (2 * PATCHES.len());
        let actual = pixel(frame, x, frame.height as usize / 2);
        assert_close(actual, *expected, &format!("{context} patch {index}"));
    }
}

const BT709_TV: &[&str] = &["-colorspace", "bt709", "-color_range", "tv"];

struct ColourCase {
    name: &'static str,
    size: (usize, usize),
    matrix: Matrix,
    full_range: bool,
    tags: &'static [&'static str],
}

struct RotationCase {
    display_rotation: &'static str,
    clockwise: u32,
    size: (usize, usize),
    left_edge_at: (usize, usize),
    right_edge_at: (usize, usize),
}

#[test]
fn decodes_known_ycbcr_with_the_tagged_matrix_and_range() {
    let cases = [
        ColourCase {
            name: "bt709_sd",
            size: (64, 48),
            matrix: BT709,
            full_range: false,
            tags: BT709_TV,
        },
        ColourCase {
            name: "bt601_hd",
            size: (1280, 720),
            matrix: BT601,
            full_range: false,
            tags: &["-colorspace", "smpte170m", "-color_range", "tv"],
        },
        ColourCase {
            name: "bt2020",
            size: (64, 48),
            matrix: BT2020,
            full_range: false,
            tags: &["-colorspace", "bt2020nc", "-color_range", "tv"],
        },
        ColourCase {
            name: "bt709_full",
            size: (64, 48),
            matrix: BT709,
            full_range: true,
            tags: &["-colorspace", "bt709", "-color_range", "pc"],
        },
        ColourCase {
            name: "untagged_sd_reads_as_bt601",
            size: (64, 48),
            matrix: BT601,
            full_range: false,
            tags: &[],
        },
        ColourCase {
            name: "untagged_hd_reads_as_bt709",
            size: (1280, 720),
            matrix: BT709,
            full_range: false,
            tags: &[],
        },
    ];
    for case in cases {
        let Some(path) = encode_patches(
            case.name,
            case.size,
            case.matrix,
            case.full_range,
            case.tags,
        ) else {
            return;
        };
        let frames = decode_all_rgba(&path);
        assert_eq!(frames.len(), 2, "{}", case.name);
        for frame in &frames {
            assert_patches(frame, case.name);
        }
        let _ = fs::remove_file(path);
    }
}

#[test]
fn keeps_the_matrix_across_a_resolution_change() {
    // Both sizes are SD, so only the tag can select BT.709 once the converter is rebuilt.
    let sizes = [(64, 48), (128, 96)];
    let mut stream = Vec::new();
    for size in sizes {
        let Some(path) = encode_patches("colour_clip", size, BT709, false, BT709_TV) else {
            return;
        };
        stream.extend(fs::read(&path).expect("read clip"));
        let _ = fs::remove_file(path);
    }
    let path = temp_path("colour_resolution_change", "h264");
    fs::write(&path, stream).expect("write stream");

    let frames = decode_all_rgba(&path);
    for (width, height) in sizes {
        assert!(
            frames
                .iter()
                .any(|frame| frame.width == width as u32 && frame.height == height as u32),
            "missing {width}x{height}"
        );
    }
    for frame in &frames {
        assert_patches(frame, &format!("{}x{}", frame.width, frame.height));
    }
    let _ = fs::remove_file(path);
}

#[test]
fn applies_display_rotation_to_stream_info_and_frames() {
    let (width, height) = (64usize, 32usize);
    let Some(coded) = encode_patches("rotation", (width, height), BT709, false, BT709_TV) else {
        return;
    };
    let [left, right] = [PATCHES[0], PATCHES[3]];
    // FFmpeg's `display_rotation` is counterclockwise. Positions match the FFmpeg CLI's autorotate.
    let cases = [
        RotationCase {
            display_rotation: "90",
            clockwise: 270,
            size: (32, 64),
            left_edge_at: (16, 60),
            right_edge_at: (16, 4),
        },
        RotationCase {
            display_rotation: "-90",
            clockwise: 90,
            size: (32, 64),
            left_edge_at: (16, 4),
            right_edge_at: (16, 60),
        },
        RotationCase {
            display_rotation: "180",
            clockwise: 180,
            size: (64, 32),
            left_edge_at: (60, 16),
            right_edge_at: (4, 16),
        },
    ];
    for case in cases {
        let path = temp_path("rotated", "mp4");
        let status = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-display_rotation", case.display_rotation, "-i"])
            .arg(&coded)
            .args(["-c", "copy"])
            .arg(&path)
            .status()
            .expect("run ffmpeg");
        assert!(status.success());

        let input = InputContext::open(path.to_string_lossy().to_string()).expect("open video");
        let stream = input.best_video_stream().expect("video stream");
        assert_eq!(stream.rotation, case.clockwise);
        assert_eq!((stream.width as usize, stream.height as usize), case.size);

        let frames = decode_all_rgba(&path);
        let frame = frames.first().expect("decoded frame");
        assert_eq!((frame.width as usize, frame.height as usize), case.size);
        assert_eq!(frame.stride, case.size.0 * 4);
        let context = format!("rotation {}", case.display_rotation);
        let (x, y) = case.left_edge_at;
        assert_close(pixel(frame, x, y), left, &context);
        let (x, y) = case.right_edge_at;
        assert_close(pixel(frame, x, y), right, &context);
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_file(coded);
}

#[test]
fn best_video_stream_prefers_the_default_track_over_stream_order() {
    let thumbnail = temp_path("thumbnail", "mp4");
    let main = temp_path("main_track", "mp4");
    for (path, source, frames) in [
        (&thumbnail, "testsrc=size=16x16:rate=30", "2"),
        (&main, "testsrc=size=64x36:rate=30", "30"),
    ] {
        let available = run_fixture(
            Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
                .args(["-i", source, "-frames:v", frames])
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                ])
                .arg(path),
            "best stream test",
        );
        if !available {
            return;
        }
    }
    let path = temp_path("two_video_tracks", "mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(&thumbnail)
        .arg("-i")
        .arg(&main)
        .args(["-map", "0:v", "-map", "1:v", "-c", "copy"])
        .args(["-disposition:v:0", "0", "-disposition:v:1", "default"])
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(status.success());

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open video");
    assert_eq!(input.media_info().video_streams[0].width, 16);
    let best = input.best_video_stream().expect("best stream");
    assert_eq!((best.stream_index, best.width), (1, 64));
    for path in [path, thumbnail, main] {
        let _ = fs::remove_file(path);
    }
}
