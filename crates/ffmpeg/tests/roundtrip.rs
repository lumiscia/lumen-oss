use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use lumen_ffmpeg::{
    AudioDecoder, AudioEncoderConfig, AudioFrame, AudioResampler, AudioResamplerConfig,
    CpuVideoFrame, DecodeMode, FfmpegError, InputContext, MuxedEncoder, PixelFormat, RateControl,
    Rational, VideoCodec, VideoDecoder, VideoDecoderConfig, VideoEncoderConfig,
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
        samples,
        interleaved_f32,
    }
}

/// A software H.264 encoder every test machine must have.
fn libx264(width: u32, height: u32, fps: impl Into<Rational>) -> VideoEncoderConfig {
    VideoEncoderConfig::cpu_rgba(width, height, fps, "libx264")
}

fn create_encoder(
    path: &Path,
    video: VideoEncoderConfig,
    audio: Option<AudioEncoderConfig>,
) -> MuxedEncoder {
    MuxedEncoder::create_with_audio(path.to_string_lossy().to_string(), video, audio)
        .expect("create encoder")
}

fn write_video_frames(encoder: &mut MuxedEncoder, width: u32, height: u32, count: i64) {
    for pts in 0..count {
        encoder
            .write_video_frame(&frame(width, height, pts))
            .expect("write video frame");
    }
}

fn write_one_second_of_audio(encoder: &mut MuxedEncoder) {
    let mut offset = 0;
    while offset < 48_000 {
        let samples = (48_000 - offset).min(1024);
        encoder
            .write_audio_frame(&audio_frame(48_000, 2, samples, offset))
            .expect("write audio frame");
        offset += samples;
    }
}

fn write_video(path: &Path, config: VideoEncoderConfig, count: i64) {
    let (width, height) = (config.width, config.height);
    let mut encoder = create_encoder(path, config, None);
    write_video_frames(&mut encoder, width, height, count);
    encoder.finish().expect("finish encode");
}

fn make_video(path: &Path) {
    write_video(path, libx264(32, 24, 24), 6);
}

fn make_muxed_video(path: &Path) {
    let audio = AudioEncoderConfig::aac(48_000, 2);
    let mut encoder = create_encoder(path, libx264(64, 36, 30), Some(audio));
    write_video_frames(&mut encoder, 64, 36, 30);
    write_one_second_of_audio(&mut encoder);
    encoder.finish().expect("finish muxed encode");
}

/// Hardware tests run only when `variable` is set; once it is, every failure fails the test.
fn hardware_test_enabled(variable: &str, what: &str) -> bool {
    let enabled = std::env::var_os(variable).is_some();
    if !enabled {
        eprintln!("set {variable}=1 to run the {what} hardware test");
    }
    enabled
}

#[cfg(all(feature = "cuda", target_os = "linux"))]
fn make_cuda_video_with_nvenc(path: &Path, width: u32, height: u32, with_audio: bool) {
    use lumen_ffmpeg::{CudaDriver, EncodeMode, GpuBackend, GpuVideoInput};

    let driver = CudaDriver::load().expect("CUDA driver");
    let context = driver.create_primary_context().expect("CUDA context");
    let frame = context
        .allocate_rgba_frame(width, height)
        .expect("CUDA frame allocation");
    frame.clear(0x7f).expect("CUDA frame clear");

    let mut video = VideoEncoderConfig::cpu_rgba(width, height, 30, "h264_nvenc");
    video.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    video.quality.rate_control = RateControl::Bitrate(2_000_000);
    let audio = with_audio.then(|| AudioEncoderConfig::aac(48_000, 2));
    let mut encoder = create_encoder(path, video, audio);
    for pts in 0..30 {
        let cuda_frame = frame.as_video_frame(Some(pts));
        let input = GpuVideoInput::Cuda(&cuda_frame);
        encoder.write_gpu_frame(&input).expect("write CUDA frame");
    }
    if with_audio {
        write_one_second_of_audio(&mut encoder);
    }
    encoder.finish().expect("finish CUDA encode");
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
    make_video(&path);

    let info = InputContext::open(path.to_string_lossy().to_string())
        .expect("open output")
        .media_info();
    assert_eq!(info.video_streams.len(), 1);
    assert_eq!(info.video_streams[0].width, 32);
    assert_eq!(info.video_streams[0].height, 24);

    let decoded = decode_all_rgba(&path);
    assert_eq!(decoded.len(), 6);
    for frame in &decoded {
        assert_eq!((frame.width, frame.height), (32, 24));
        assert_eq!(frame.stride, 32 * 4);
        assert_eq!(frame.data.len(), 32 * 24 * 4);
    }

    assert_bt709_tags(&path);
    let _ = fs::remove_file(path);
}

#[test]
fn encodes_muxed_video_and_audio() {
    let path = temp_path("muxed_roundtrip", "mp4");
    make_muxed_video(&path);

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let info = input.media_info();
    assert_eq!(info.video_streams.len(), 1);
    assert_eq!(info.audio_streams.len(), 1);
    assert_eq!(info.video_streams[0].codec, VideoCodec::H264);
    assert_eq!(info.video_streams[0].width, 64);
    assert_eq!(info.video_streams[0].height, 36);
    assert_eq!(info.audio_streams[0].sample_rate, 48_000);
    assert_eq!(info.audio_streams[0].channels, 2);
    assert_eq!(decode_all_rgba(&path).len(), 30);

    let _ = fs::remove_file(path);
}

#[test]
fn encodes_cpu_frames_with_nvenc_when_requested() {
    if !hardware_test_enabled("LUMEN_FFMPEG_TEST_NVENC", "NVENC") {
        return;
    }

    let path = temp_path("nvenc_roundtrip", "mp4");
    let mut config = VideoEncoderConfig::cpu_rgba(640, 360, 30, "h264_nvenc");
    config.quality.rate_control = RateControl::Bitrate(2_000_000);
    write_video(&path, config, 30);

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
    if !hardware_test_enabled("LUMEN_FFMPEG_TEST_CUDA_NVENC", "CUDA NVENC") {
        return;
    }

    for with_audio in [false, true] {
        let path = temp_path("cuda_nvenc_roundtrip", "mp4");
        make_cuda_video_with_nvenc(&path, 640, 360, with_audio);

        let input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
        let info = input.media_info();
        assert_eq!(info.video_streams.len(), 1, "audio {with_audio}");
        assert_eq!(info.video_streams[0].codec, VideoCodec::H264);
        assert_eq!(info.video_streams[0].width, 640);
        assert_eq!(info.video_streams[0].height, 360);
        assert_eq!(info.audio_streams.len(), usize::from(with_audio));
        if with_audio {
            assert_eq!(info.audio_streams[0].sample_rate, 48_000);
            assert_eq!(info.audio_streams[0].channels, 2);
        }

        let _ = fs::remove_file(path);
    }
}

#[cfg(all(feature = "cuda", target_os = "linux"))]
#[test]
fn cuda_nvenc_keeps_each_frame_when_the_source_buffer_is_reused() {
    use lumen_ffmpeg::{CudaDriver, EncodeMode, GpuBackend, GpuVideoInput};

    if !hardware_test_enabled("LUMEN_FFMPEG_TEST_CUDA_NVENC", "CUDA NVENC") {
        return;
    }

    let (width, height, count) = (640, 360, 120);
    let driver = CudaDriver::load().expect("CUDA driver");
    let context = driver.create_primary_context().expect("CUDA context");
    let source = context
        .allocate_rgba_frame(width, height)
        .expect("CUDA frame allocation");

    let path = temp_path("cuda_nvenc_order", "mp4");
    let mut video = VideoEncoderConfig::cpu_rgba(width, height, 30, "h264_nvenc");
    video.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    video.quality.rate_control = RateControl::Bitrate(4_000_000);
    let mut encoder = create_encoder(&path, video, None);
    for index in 0..count {
        source.clear(index as u8 * 2).expect("CUDA frame clear");
        let frame = source.as_video_frame(Some(index));
        encoder
            .write_gpu_frame(&GpuVideoInput::Cuda(&frame))
            .expect("write CUDA frame");
    }
    encoder.finish().expect("finish CUDA encode");

    let frames = decode_all_rgba(&path);
    assert_eq!(frames.len(), count as usize);
    for (index, frame) in frames.iter().enumerate() {
        let expected = index as u8 * 2;
        let [red, ..] = pixel(frame, width as usize / 2, height as usize / 2);
        assert!(
            red.abs_diff(expected) <= 6,
            "frame {index} holds the content of another frame: red {red}, expected {expected}"
        );
    }
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
            assert!(
                frame
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| pixel[3] == 255)
            );
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
    write_video(&path, libx264(32, 24, Rational::new(30_000, 1001)), 30);

    let input = InputContext::open(path.to_string_lossy().to_string()).expect("open output");
    let rate = input.media_info().video_streams[0].avg_frame_rate;
    assert_eq!(rate.numerator * 1001, rate.denominator * 30_000);

    let _ = fs::remove_file(path);
}

fn noisy_frame(width: u32, height: u32, seed: u32) -> CpuVideoFrame {
    let stride = width as usize * 4;
    let data = (0..stride * height as usize)
        .map(|index| {
            (index as u32)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(seed) as u8
                >> 3
        })
        .collect();
    CpuVideoFrame {
        width,
        height,
        stride,
        pixel_format: PixelFormat::Rgba8,
        pts: Some(i64::from(seed)),
        data,
    }
}

fn encode_noise(path: &Path, config: VideoEncoderConfig) -> Result<(), FfmpegError> {
    let (width, height) = (config.width, config.height);
    let mut encoder = MuxedEncoder::create(path.to_string_lossy().to_string(), config)?;
    for index in 0..10 {
        encoder.write_video_frame(&noisy_frame(width, height, index))?;
    }
    encoder.finish()
}

#[test]
fn libx264_crf_controls_output_size() {
    let mut sizes = Vec::new();
    for crf in [18, 40] {
        let path = temp_path("x264_crf", "mp4");
        let mut config = libx264(128, 72, 30);
        config.quality.rate_control = RateControl::Crf(crf);
        config.quality.preset = Some("ultrafast".to_string());
        encode_noise(&path, config).expect("libx264 CRF encode");
        let info = InputContext::open(path.to_string_lossy().to_string())
            .expect("open encoded file")
            .media_info();
        assert_eq!(info.video_streams[0].codec, VideoCodec::H264);
        sizes.push(fs::metadata(&path).expect("encoded file").len());
        let _ = fs::remove_file(path);
    }
    assert!(sizes[0] > sizes[1], "lower CRF should be larger: {sizes:?}");
}

const VULKAN_TEST_VARIABLE: &str = "LUMEN_FFMPEG_TEST_VULKAN";

fn av1_vulkan(width: u32, height: u32) -> VideoEncoderConfig {
    VideoEncoderConfig::cpu_rgba(width, height, 30, "av1_vulkan")
}

/// Needs Vulkan AV1 encode and decode. On RADV, set `RADV_PERFTEST=video_encode,video_decode`.
#[test]
fn vulkan_encoder_uploads_cpu_frames() {
    if !hardware_test_enabled(VULKAN_TEST_VARIABLE, "Vulkan encode") {
        return;
    }
    const FRAMES: usize = 10;
    let (width, height) = (256_usize, 144_usize);
    let path = temp_path("vulkan_upload", "mkv");
    let mut encoder = MuxedEncoder::create(
        path.to_string_lossy().to_string(),
        av1_vulkan(width as u32, height as u32),
    )
    .expect("create av1_vulkan encoder");
    let mut patches = Vec::with_capacity(width * height * 4);
    for _ in 0..height {
        for x in 0..width {
            let [r, g, b] = PATCHES[x * PATCHES.len() / width];
            patches.extend([r, g, b, 255]);
        }
    }
    for index in 0..FRAMES {
        encoder
            .write_video_frame(&CpuVideoFrame {
                data: patches.clone(),
                ..noisy_frame(width as u32, height as u32, index as u32)
            })
            .expect("upload and encode frame");
    }
    encoder.finish().expect("finish encode");

    let stream = InputContext::open(path.to_string_lossy().to_string())
        .expect("open file")
        .media_info()
        .video_streams[0]
        .clone();
    assert_eq!(stream.codec, VideoCodec::Av1);
    assert_eq!(
        (stream.width as usize, stream.height as usize),
        (width, height)
    );

    let raw = temp_path("vulkan_upload", "rgba");
    assert!(
        run_fixture(
            Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error"])
                .args(["-init_hw_device", "vulkan=gpu", "-hwaccel", "vulkan"])
                .args(["-hwaccel_output_format", "vulkan", "-hwaccel_device", "gpu"])
                .arg("-i")
                .arg(&path)
                .args(["-vf", "hwdownload,format=nv12,format=rgba"])
                .args(["-f", "rawvideo"])
                .arg(&raw),
            "decode av1_vulkan output",
        ),
        "the ffmpeg CLI is required to check {VULKAN_TEST_VARIABLE} output"
    );
    let decoded = fs::read(&raw).expect("read decoded frames");
    assert_eq!(
        decoded.len(),
        FRAMES * width * height * 4,
        "flush must emit every submitted frame"
    );
    let last_frame = &decoded[(FRAMES - 1) * width * height * 4..];
    for (index, expected) in PATCHES.iter().enumerate() {
        let x = (2 * index + 1) * width / (2 * PATCHES.len());
        let offset = (height / 2 * width + x) * 4;
        let actual = &last_frame[offset..offset + 3];
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, e)| (i32::from(*a) - i32::from(*e)).abs() <= 12),
            "patch {index}: decoded {actual:?}, expected {expected:?}"
        );
    }

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(raw);
}

#[test]
fn vulkan_encoder_supports_qp_and_rejects_crf() {
    if !hardware_test_enabled(VULKAN_TEST_VARIABLE, "Vulkan encode") {
        return;
    }
    let path = temp_path("vulkan_qp", "mkv");
    let mut config = av1_vulkan(256, 144);
    config.quality.rate_control = RateControl::Crf(23);
    let error = match MuxedEncoder::create(path.to_string_lossy().to_string(), config.clone()) {
        Ok(_) => panic!("av1_vulkan has no CRF mode"),
        Err(error) => error,
    };
    assert!(error.message.contains("CRF"), "{error}");

    config.quality.rate_control = RateControl::Qp(100);
    encode_noise(&path, config).expect("av1_vulkan QP encode");
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
/// installed; any other failure, including a missing encoder, panics so a broken fixture can't pass.
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

#[test]
fn decodes_every_column_when_the_width_is_not_a_multiple_of_eight() {
    let formats = [
        ("yuv420p", [854, 1022, 1366].as_slice()),
        ("yuv422p", [854, 1022].as_slice()),
        ("yuv444p", [853, 855, 854].as_slice()),
        ("yuv420p10le", [854, 1022].as_slice()),
    ];
    for (format, widths) in formats {
        for &width in widths {
            let name = format!("columns_{format}_{width}");
            let path = temp_path(&name, "mp4");
            let available = run_fixture(
                Command::new("ffmpeg")
                    .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
                    .arg("-i")
                    .arg(format!("color=c=0xc86432:s={width}x16:r=30:d=0.1"))
                    .args([
                        "-pix_fmt",
                        format,
                        "-c:v",
                        "libx264",
                        "-preset",
                        "ultrafast",
                    ])
                    .arg(&path),
                &name,
            );
            if !available {
                return;
            }
            let frames = decode_all_rgba(&path);
            assert!(!frames.is_empty(), "{name}");
            for frame in &frames {
                assert_eq!(frame.width as usize, width, "{name}");
                let expected = &frame.data[..4];
                assert_eq!(expected[3], 255, "{name}");
                for (index, pixel) in frame.data.as_chunks::<4>().0.iter().enumerate() {
                    assert_eq!(
                        pixel,
                        expected,
                        "{name}: column {} of row {}",
                        index % width,
                        index / width
                    );
                }
            }
            let _ = fs::remove_file(path);
        }
    }
}
