//! Renders each output kind on the machine's GPU and reads the results back.

use std::{fs, path::Path, sync::MutexGuard};

use lumen_engine::composition::Composition;
use lumen_ffmpeg::{
    AudioRangeReader, DecodeMode, DecodedVideoFrame, EncoderQuality, InputContext, VideoDecoder,
    VideoDecoderConfig, VideoEncoderChoice, YuvVideoFrame,
};
use serde_json::json;

use super::{
    CancelToken, ErrorCode, FrameRange, OutputFormat, RenderConfig, RenderOptions, RenderOutput,
    SequenceLimits, archive::tests::read_zip, parse_composition, render_project,
};

const FPS: u32 = 24;
const FRAMES: u32 = 72;
const SAMPLE_RATE: usize = 48_000;
/// The sample the range `24..48` starts at: one second in.
const RANGE_START: usize = SAMPLE_RATE;

/// A 16 x 16 composition whose red grows by a 30th of the way to full every frame, with a ramp of
/// audio under it when `audio` is set: sample `n` of the voice is `n / 144000 * 0.8`, so the
/// level of the audio tells where in the timeline it was taken from.
fn ramp(audio: bool) -> Composition {
    let mut composition = json!({
        "timeline": { "fps": FPS, "duration_frames": FRAMES },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": [
            {
                "id": 1,
                "type": "background",
                "params": {
                    "paint": "=tween(#000000, #ff0000, 0, 30, 'linear')",
                    "width": 16,
                    "height": 16
                }
            },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    });
    if audio {
        composition["audio"] = voice();
    }
    parse_composition(&composition).unwrap()
}

/// Three seconds of the voice from time 0.
fn voice() -> serde_json::Value {
    json!({
        "tracks": [{ "id": "t", "name": "t", "muted": false, "solo": false, "volume": 1.0 }],
        "clips": [{
            "id": "c", "source_id": "voice.wav", "track_id": "t", "name": "voice.wav",
            "start_ms": 0, "duration_ms": 3000, "source_start_ms": 0, "volume": 1.0
        }]
    })
}

fn transparent() -> Composition {
    parse_composition(&json!({
        "timeline": { "fps": FPS, "duration_frames": 2 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 128], "width": 16, "height": 16 } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    }))
    .unwrap()
}

fn write_voice(directory: &Path) {
    let total = SAMPLE_RATE * 3;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((36 + total as u32 * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16_u32.to_le_bytes());
    wav.extend([1, 0, 1, 0]); // PCM, mono
    wav.extend((SAMPLE_RATE as u32).to_le_bytes());
    wav.extend((SAMPLE_RATE as u32 * 2).to_le_bytes());
    wav.extend([2, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend((total as u32 * 2).to_le_bytes());
    for sample in 0..total {
        let level = sample as f32 / (3 * SAMPLE_RATE) as f32 * 0.8;
        wav.extend(((level * 32767.0) as i16).to_le_bytes());
    }
    fs::write(directory.join("voice.wav"), wav).unwrap();
}

struct Setup {
    _serial: MutexGuard<'static, ()>,
    media: tempfile::TempDir,
    temp: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let serial = crate::render::gpu_test_guard();
        let media = tempfile::tempdir().unwrap();
        write_voice(media.path());
        Self {
            _serial: serial,
            media,
            temp: tempfile::tempdir().unwrap(),
        }
    }

    fn options(&self, output: RenderOutput) -> RenderOptions {
        RenderOptions {
            config: RenderConfig {
                temp_dir: Some(self.temp.path().to_path_buf()),
                ..RenderConfig::default()
            },
            media_root: Some(self.media.path().to_path_buf()),
            encoder: Some(VideoEncoderChoice::single(
                "libx264",
                EncoderQuality::default(),
            )),
            output,
            ..RenderOptions::default()
        }
    }

    fn render(&self, composition: &Composition, output: RenderOutput) -> Vec<u8> {
        render_project(composition, &self.options(output), &mut |_| {})
            .unwrap()
            .bytes
    }

    fn nothing_left_behind(&self) -> bool {
        fs::read_dir(self.temp.path()).unwrap().count() == 0
    }
}

fn range(start: Option<u32>, end: Option<u32>) -> FrameRange {
    FrameRange { start, end }
}

fn video(frames: FrameRange) -> RenderOutput {
    RenderOutput {
        frames,
        ..RenderOutput::default()
    }
}

fn image_format(format: OutputFormat, frames: FrameRange) -> RenderOutput {
    RenderOutput {
        format,
        frames,
        jpeg_quality: None,
    }
}

/// The frames of a video, as the YUV planes it stores, and when the first one is shown.
fn decode(bytes: &[u8], directory: &Path) -> (Vec<YuvVideoFrame>, i64) {
    let path = directory.join("decoded.mp4");
    fs::write(&path, bytes).unwrap();
    let mut input = InputContext::open(path.to_string_lossy().to_string()).unwrap();
    let stream = input.best_video_stream().unwrap();
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index: stream.stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .unwrap();
    let mut frames = Vec::new();
    let mut first_pts = i64::MAX;
    let mut take = |decoder: &mut VideoDecoder| {
        while let Some(frame) = decoder.receive_video_frame().unwrap() {
            let DecodedVideoFrame::Yuv(frame) = frame else {
                panic!("the encoder writes a YUV stream");
            };
            first_pts = first_pts.min(frame.pts.unwrap_or(i64::MAX));
            frames.push(frame);
        }
    };
    while let Some(packet) = input.read_packet().unwrap() {
        decoder.send_packet(&packet).unwrap();
        take(&mut decoder);
    }
    decoder.send_eof().unwrap();
    take(&mut decoder);
    (frames, first_pts)
}

/// The video's duration in seconds, and the number of samples its audio holds.
fn durations(bytes: &[u8], directory: &Path) -> (f64, usize) {
    let path = directory.join("durations.mp4");
    fs::write(&path, bytes).unwrap();
    let input = InputContext::open(path.to_string_lossy().to_string()).unwrap();
    let video = input.best_video_stream().unwrap();
    let seconds = video.duration_ts.unwrap() as f64 * video.time_base.numerator as f64
        / video.time_base.denominator as f64;
    let mut audio = AudioRangeReader::open(path.to_string_lossy().to_string()).unwrap();
    (seconds, audio.read(0, 4 * SAMPLE_RATE).unwrap().samples)
}

/// The mean of the left channel over `len` samples from `start`.
fn mean_level(bytes: &[u8], directory: &Path, start: usize, len: usize) -> f32 {
    let path = directory.join("levels.mp4");
    fs::write(&path, bytes).unwrap();
    let mut audio = AudioRangeReader::open(path.to_string_lossy().to_string()).unwrap();
    let frame = audio.read(start as u64, len).unwrap();
    assert_eq!(frame.samples, len);
    frame
        .interleaved_f32
        .chunks(frame.channels as usize)
        .map(|samples| samples[0])
        .sum::<f32>()
        / len as f32
}

/// BT.709 limited-range luma of an RGB pixel, which is what the video stores.
fn luma(rgb: [u8; 3]) -> f32 {
    let [r, g, b] = rgb.map(f32::from);
    16.0 + 219.0 * (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}

#[test]
fn a_ranged_video_holds_exactly_those_frames_and_audio_from_time_zero() {
    let setup = Setup::new();
    let composition = ramp(true);
    let full = setup.render(&composition, RenderOutput::default());
    let ranged = setup.render(&composition, video(range(Some(24), Some(48))));
    let scratch = tempfile::tempdir().unwrap();

    let (full_frames, _) = decode(&full, scratch.path());
    let (ranged_frames, first_pts) = decode(&ranged, scratch.path());
    assert_eq!(full_frames.len(), FRAMES as usize);
    assert_eq!(ranged_frames.len(), 24);
    assert_eq!(first_pts, 0, "the output starts at time 0");
    // The first frame of the range is the full render's frame at its start.
    for (ranged, full) in [(0, 24), (23, 47)] {
        let (got, want) = (
            ranged_frames[ranged].sample(8, 8)[0] as f32,
            full_frames[full].sample(8, 8)[0] as f32,
        );
        assert!((got - want).abs() <= 6.0, "frame {full}: {got} vs {want}");
    }

    let (full_seconds, full_samples) = durations(&full, scratch.path());
    let (seconds, samples) = durations(&ranged, scratch.path());
    assert!((full_seconds - 3.0).abs() < 1e-6, "{full_seconds}");
    assert!((seconds - 1.0).abs() < 1e-6, "{seconds}");
    // One second of audio, give or take the encoder's padding of the last AAC frame.
    assert!(
        samples.abs_diff(SAMPLE_RATE) <= 1100,
        "{samples} samples of audio"
    );
    assert!(
        full_samples.abs_diff(3 * SAMPLE_RATE) <= 1100,
        "{full_samples}"
    );

    // The audio is the full render's from the range's start, with no silence before it.
    let ranged_start = mean_level(&ranged, scratch.path(), 2_000, 2_000);
    let full_start = mean_level(&full, scratch.path(), RANGE_START + 2_000, 2_000);
    assert!(full_start > 0.1, "{full_start}");
    assert!(
        (ranged_start - full_start).abs() < 0.02,
        "{ranged_start} vs {full_start}"
    );
    let ranged_end = mean_level(&ranged, scratch.path(), 44_000, 2_000);
    let full_end = mean_level(&full, scratch.path(), RANGE_START + 44_000, 2_000);
    assert!(
        (ranged_end - full_end).abs() < 0.02,
        "{ranged_end} vs {full_end}"
    );
    assert!(setup.nothing_left_behind());
}

#[test]
fn a_range_open_at_one_end_runs_to_the_timeline() {
    let setup = Setup::new();
    let composition = ramp(false);
    let scratch = tempfile::tempdir().unwrap();
    let tail = setup.render(&composition, video(range(Some(60), None)));
    assert_eq!(decode(&tail, scratch.path()).0.len(), 12);
    let head = setup.render(&composition, video(range(None, Some(5))));
    assert_eq!(decode(&head, scratch.path()).0.len(), 5);
}

#[test]
fn progress_counts_the_frames_of_the_range() {
    let setup = Setup::new();
    let mut seen = Vec::new();
    render_project(
        &ramp(false),
        &setup.options(image_format(
            OutputFormat::PngSequence,
            range(Some(10), Some(14)),
        )),
        &mut |progress| seen.push((progress.frame, progress.total_frames)),
    )
    .unwrap();
    assert_eq!(seen, [(1, 4), (2, 4), (3, 4), (4, 4)]);
}

#[test]
fn a_range_the_timeline_does_not_have_is_refused_before_any_gpu_work() {
    let setup = Setup::new();
    let composition = ramp(false);
    for format in [
        OutputFormat::Mp4,
        OutputFormat::PngSequence,
        OutputFormat::Png,
    ] {
        for frames in [
            range(Some(5), Some(5)),
            range(Some(9), Some(3)),
            range(Some(FRAMES), None),
            range(None, Some(FRAMES + 1)),
        ] {
            let mut options = setup.options(image_format(format, frames));
            // An encoder that doesn't exist would fail the render if it got as far as opening one.
            options.encoder = Some(VideoEncoderChoice::single(
                "definitely_not_an_encoder",
                EncoderQuality::default(),
            ));
            let error = render_project(&composition, &options, &mut |_| panic!("nothing renders"))
                .unwrap_err();
            assert_eq!(
                error.code(),
                ErrorCode::InvalidOutput,
                "{format:?} {frames:?}"
            );
            assert!(!error.retryable());
        }
    }
    assert!(setup.nothing_left_behind());
}

#[test]
fn stills_decode_to_the_composition_size_and_match_the_video() {
    let setup = Setup::new();
    let composition = ramp(false);
    let scratch = tempfile::tempdir().unwrap();
    let (frames, _) = decode(
        &setup.render(&composition, RenderOutput::default()),
        scratch.path(),
    );

    for frame in [0, 9, 29] {
        let at = range(Some(frame), None);
        let png = setup.render(&composition, image_format(OutputFormat::Png, at));
        let png = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!((png.width(), png.height()), (16, 16));
        let pixel = png.to_rgba8().get_pixel(8, 8).0;
        assert_eq!(pixel[3], 255);
        let video = frames[frame as usize].sample(8, 8)[0] as f32;
        assert!(
            (luma([pixel[0], pixel[1], pixel[2]]) - video).abs() <= 5.0,
            "png frame {frame}: {pixel:?} vs {video}"
        );

        let jpeg = setup.render(&composition, image_format(OutputFormat::Jpeg, at));
        let jpeg = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((jpeg.width(), jpeg.height()), (16, 16));
        let pixel = jpeg.to_rgb8().get_pixel(8, 8).0;
        assert!(
            (luma(pixel) - video).abs() <= 8.0,
            "jpeg frame {frame}: {pixel:?} vs {video}"
        );
    }
    let default = setup.render(
        &composition,
        image_format(OutputFormat::Png, FrameRange::default()),
    );
    let first = setup.render(
        &composition,
        image_format(OutputFormat::Png, range(Some(0), Some(1))),
    );
    assert_eq!(default, first, "a still defaults to frame 0");
}

#[test]
fn a_png_keeps_transparency_and_a_jpeg_is_that_colour_over_black() {
    let setup = Setup::new();
    let composition = transparent();
    let png = setup.render(&composition, RenderOutput::of(OutputFormat::Png));
    let pixel = image::load_from_memory(&png)
        .unwrap()
        .to_rgba8()
        .get_pixel(8, 8)
        .0;
    assert!(pixel[3].abs_diff(128) <= 1, "{pixel:?}");
    assert!(
        pixel[0] >= 254 && pixel[1] <= 1 && pixel[2] <= 1,
        "{pixel:?}"
    );

    let jpeg = setup.render(&composition, RenderOutput::of(OutputFormat::Jpeg));
    let pixel = image::load_from_memory(&jpeg)
        .unwrap()
        .to_rgb8()
        .get_pixel(8, 8)
        .0;
    // Half-covered red over black is half the light of red: code 188.
    assert!(
        pixel[0].abs_diff(188) <= 6 && pixel[1] <= 6 && pixel[2] <= 6,
        "{pixel:?}"
    );
}

#[test]
fn a_png_keeps_the_straight_colour_of_faint_pixels() {
    // White at opacity 0.002 has alpha 0.5 of a code: every pixel the PNG keeps is still white,
    // and none is colour without alpha.
    let setup = Setup::new();
    let composition = parse_composition(&json!({
        "timeline": { "fps": FPS, "duration_frames": 1 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paint": [255, 255, 255, 255] } },
            { "id": 2, "type": "opacity", "params": { "opacity": 0.002 } },
            { "id": 3, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 2, "to_port": "source" },
            { "from_node": 2, "to_node": 3, "to_port": "source" }
        ]
    }))
    .unwrap();
    let png = setup.render(&composition, RenderOutput::of(OutputFormat::Png));
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        assert!(
            (a > 0 && [r, g, b] == [255; 3]) || pixel.0 == [0; 4],
            "{:?}",
            pixel.0
        );
    }
}

#[test]
fn jpeg_quality_changes_the_size() {
    let setup = Setup::new();
    let composition = parse_composition(&json!({
        "timeline": { "fps": FPS, "duration_frames": 1 },
        "render_settings": { "width": 128, "height": 128 },
        "nodes": [
            {
                "id": 1,
                "type": "background",
                "params": {
                    "paint": {
                        "type": "linear_gradient",
                        "start": [0, 0],
                        "end": [1, 1],
                        "stops": [
                            { "offset": 0, "color": [255, 0, 0, 255] },
                            { "offset": 0.3, "color": [0, 255, 40, 255] },
                            { "offset": 0.6, "color": [10, 0, 255, 255] },
                            { "offset": 1, "color": [250, 204, 21, 255] }
                        ]
                    },
                    "width": 128,
                    "height": 128
                }
            },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    }))
    .unwrap();
    let at = |quality| RenderOutput {
        jpeg_quality: Some(quality),
        ..RenderOutput::of(OutputFormat::Jpeg)
    };
    let low = setup.render(&composition, at(5));
    let high = setup.render(&composition, at(100));
    assert!(low.len() < high.len(), "{} vs {}", low.len(), high.len());
}

#[test]
fn a_sequence_is_a_zip_of_numbered_frames_that_match_the_stills() {
    let setup = Setup::new();
    let composition = ramp(false);
    for (format, extension, signature) in [
        (OutputFormat::PngSequence, "png", &b"\x89PNG"[..]),
        (OutputFormat::JpegSequence, "jpg", &b"\xff\xd8"[..]),
    ] {
        let zip = setup.render(&composition, image_format(format, range(Some(3), Some(8))));
        let files = read_zip(&zip);
        let names: Vec<_> = files.iter().map(|(name, _)| name.as_str()).collect();
        let expected: Vec<_> = (3..8)
            .map(|n| format!("frame_{n:04}.{extension}"))
            .collect();
        assert_eq!(names, expected);
        for (frame, (_, bytes)) in (3..8).zip(&files) {
            assert!(bytes.starts_with(signature));
            let still = setup.render(
                &composition,
                image_format(
                    if extension == "png" {
                        OutputFormat::Png
                    } else {
                        OutputFormat::Jpeg
                    },
                    range(Some(frame), None),
                ),
            );
            assert_eq!(bytes, &still, "frame {frame}");
        }
    }
    assert!(setup.nothing_left_behind());
}

#[test]
fn a_sequence_past_its_limits_fails_without_leaving_anything() {
    let setup = Setup::new();
    let composition = ramp(false);
    let output = image_format(OutputFormat::PngSequence, range(Some(0), Some(10)));

    let mut options = setup.options(output.clone());
    options.sequence_limits = SequenceLimits {
        max_frames: 9,
        max_bytes: u64::MAX,
    };
    let error = render_project(&composition, &options, &mut |_| panic!("too long")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::OutputTooLarge);

    // Each frame is a few hundred bytes, so ten of them pass 1 KiB part way through.
    options.sequence_limits = SequenceLimits {
        max_frames: 10,
        max_bytes: 1024,
    };
    let mut rendered = 0;
    let error = render_project(&composition, &options, &mut |progress| {
        rendered = progress.frame
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::OutputTooLarge, "{error}");
    assert!(!error.retryable());
    assert!(rendered > 0 && rendered < 10, "{rendered}");

    options.sequence_limits = SequenceLimits::default();
    assert!(render_project(&composition, &options, &mut |_| {}).is_ok());
    assert!(setup.nothing_left_behind());
}

#[test]
fn cancelling_a_sequence_stops_it_and_leaves_nothing_behind() {
    let setup = Setup::new();
    let cancel = CancelToken::new();
    let mut options = setup.options(image_format(
        OutputFormat::PngSequence,
        range(Some(0), Some(60)),
    ));
    options.cancel = cancel.clone();

    let mut rendered = 0;
    let error = render_project(&ramp(false), &options, &mut |progress| {
        rendered = progress.frame;
        if progress.frame == 3 {
            cancel.cancel();
        }
    })
    .unwrap_err();

    assert_eq!(error.code(), ErrorCode::Cancelled, "{error}");
    assert_eq!(rendered, 3);
    assert!(setup.nothing_left_behind());
}

#[test]
fn quality_settings_do_not_apply_to_images() {
    let setup = Setup::new();
    let mut options = setup.options(image_format(OutputFormat::Png, FrameRange::default()));
    options.encoder = Some(VideoEncoderChoice::single(
        "definitely_not_an_encoder",
        EncoderQuality::default(),
    ));
    assert!(render_project(&ramp(false), &options, &mut |_| {}).is_ok());
}

/// A 1024 x 64 composition, sRGB-encoded black to 30% grey from left to right.
fn dark_gradient() -> Composition {
    parse_composition(&json!({
        "timeline": { "fps": FPS, "duration_frames": 2 },
        "render_settings": { "width": 1024, "height": 64 },
        "nodes": [
            { "id": 1, "type": "background", "params": {
                "paint": {
                    "type": "linear_gradient", "start": [0, 0], "end": [1, 0],
                    "stops": [[0, [0, 0, 0, 255]], [1, [77, 77, 77, 255]]]
                },
                "width": 1024, "height": 64, "paint_supersample": false
            } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    }))
    .unwrap()
}

#[test]
fn a_ten_bit_video_keeps_more_luma_levels_than_eight_bits_have() {
    let setup = Setup::new();
    let scratch = tempfile::tempdir().unwrap();
    let lossless = EncoderQuality {
        options: vec![("x265-params".to_string(), "lossless=1".to_string())],
        ..EncoderQuality::default()
    };
    let mut rendered = 0;
    for (encoder, quality) in [
        ("libx265", lossless),
        ("libsvtav1", EncoderQuality::default()),
    ] {
        if lumen_ffmpeg::encoder_info(encoder).is_err() {
            eprintln!("{encoder} isn't in this FFmpeg build; skipping its 10-bit render");
            continue;
        }
        let mut options = setup.options(RenderOutput::default());
        options.encoder = Some(
            VideoEncoderChoice::single(encoder, quality)
                .with_bit_depth(lumen_ffmpeg::BitDepth::Ten),
        );
        let bytes = render_project(&dark_gradient(), &options, &mut |_| {})
            .unwrap()
            .bytes;
        let path = scratch.path().join(format!("{encoder}.mp4"));
        fs::write(&path, &bytes).unwrap();
        let input = InputContext::open(path.to_string_lossy().to_string()).unwrap();
        assert_eq!(
            input.best_video_stream().unwrap().pixel_format,
            lumen_ffmpeg::PixelFormat::Yuv420p10,
            "{encoder}"
        );
        rendered += 1;
        if encoder != "libx265" {
            continue;
        }
        let (frames, _) = decode(&bytes, scratch.path());
        let frame = &frames[0];
        assert_eq!(frame.depth, 10);
        let levels: std::collections::BTreeSet<u16> =
            (0..frame.width).map(|x| frame.sample(x, 32)[0]).collect();
        // Eight bits hold 67 codes from black to 30% grey, ten bits 264.
        assert!(levels.len() > 256, "{} luma levels", levels.len());
        assert!(
            levels.iter().all(|code| (64..=330).contains(code)),
            "{levels:?}"
        );
    }
    if rendered == 0 {
        eprintln!("no 10-bit software encoder in this FFmpeg build");
    }
}

#[test]
fn a_ten_bit_render_fails_where_the_encoder_cannot_rather_than_writing_eight_bits() {
    let setup = Setup::new();
    let mut options = setup.options(RenderOutput::default());
    options.encoder = Some(
        VideoEncoderChoice::single("libx264", EncoderQuality::default())
            .with_bit_depth(lumen_ffmpeg::BitDepth::Ten),
    );
    let Err(error) = render_project(&dark_gradient(), &options, &mut |_| {}) else {
        panic!("H.264 can't be encoded at 10 bits");
    };
    assert_eq!(error.code(), ErrorCode::EncodeFailed);
    assert!(error.to_string().contains("always 8-bit"), "{error}");
    assert!(setup.nothing_left_behind());
}

/// A 64 x 64 composition of nothing but an anti-aliased disc of orange at 70% opacity, with the
/// voice under it.
fn translucent_disc() -> Composition {
    let mut composition = json!({
        "timeline": { "fps": FPS, "duration_frames": 3 },
        "render_settings": { "width": 64, "height": 64 },
        "nodes": [
            { "id": 1, "type": "shape", "params": {
                "position": [32, 32], "width": 41, "height": 41, "border_radius": 20.5,
                "fill_paint": [255, 160, 32, 179]
            } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    });
    composition["audio"] = voice();
    parse_composition(&composition).unwrap()
}

/// Straight sRGB-encoded colour and alpha, 0 to 1, of a decoded BT.709 limited-range frame with
/// alpha at `(x, y)`, using the chroma sample that covers it.
fn straight_rgba(frame: &YuvVideoFrame, x: u32, y: u32) -> [f64; 4] {
    let scale = f64::from(1u32 << (frame.depth - 8));
    let [luma, cb, cr] = frame.sample(x, y).map(|code| f64::from(code) / scale);
    let (kr, kb) = (0.2126, 0.0722);
    let y_ = (luma - 16.0) / 219.0;
    let (pb, pr) = ((cb - 128.0) / 224.0, (cr - 128.0) / 224.0);
    let r = y_ + 2.0 * (1.0 - kr) * pr;
    let b = y_ + 2.0 * (1.0 - kb) * pb;
    let g = (y_ - kr * r - kb * b) / (1.0 - kr - kb);
    let (width, height) = (frame.width as usize, frame.height as usize);
    let bytes = if frame.depth > 8 { 2 } else { 1 };
    let chroma = match frame.subsampling {
        lumen_ffmpeg::YuvSubsampling::Yuv444 => width * height,
        _ => width.div_ceil(2) * height.div_ceil(2),
    };
    let at = (width * height + 2 * chroma + y as usize * width + x as usize) * bytes;
    let alpha = match bytes {
        1 => u16::from(frame.data[at]),
        _ => u16::from_le_bytes([frame.data[at], frame.data[at + 1]]),
    };
    let top = f64::from((1u32 << frame.depth) - 1);
    [r, g, b, f64::from(alpha) / top]
}

#[test]
fn alpha_videos_match_the_png_in_straight_colour_with_no_dark_fringe() {
    let setup = Setup::new();
    let scratch = tempfile::tempdir().unwrap();
    let composition = translucent_disc();
    let png = setup.render(&composition, RenderOutput::of(OutputFormat::Png));
    let png = image::load_from_memory(&png).unwrap().to_rgba8();
    let edges = png
        .pixels()
        .filter(|pixel| (1..179).contains(&pixel.0[3]))
        .count();
    assert!(edges > 40, "the disc has {edges} anti-aliased pixels");

    let lossless_vp9 = EncoderQuality {
        options: vec![("lossless".to_string(), "1".to_string())],
        ..EncoderQuality::default()
    };
    // ProRes is near-lossless 4:4:4 with lossless alpha; lossless VP9 leaves the 4:2:0 chroma,
    // which on a disc of one colour is that colour wherever alpha weights it.
    for (encoder, quality, format, depth, colour_tolerance, alpha_tolerance) in [
        (
            "prores_ks",
            EncoderQuality::default(),
            OutputFormat::Mov,
            lumen_ffmpeg::BitDepth::Ten,
            3.0,
            1.0,
        ),
        (
            "libvpx-vp9",
            lossless_vp9,
            OutputFormat::Webm,
            lumen_ffmpeg::BitDepth::Eight,
            3.0,
            1.0,
        ),
    ] {
        if lumen_ffmpeg::encoder_info(encoder).is_err() {
            eprintln!("{encoder} isn't in this FFmpeg build; skipping its alpha round trip");
            continue;
        }
        let mut options = setup.options(RenderOutput::of(format));
        options.encoder = Some(
            VideoEncoderChoice::single(encoder, quality)
                .with_bit_depth(depth)
                .with_alpha(true),
        );
        let rendered = match render_project(&composition, &options, &mut |_| {}) {
            Ok(rendered) => rendered,
            Err(error) => {
                let message = format!("{:#}", anyhow::Error::new(error));
                // WebM audio is Opus, which needs libopus.
                if message.contains("libopus") {
                    eprintln!("skipping {encoder}: {message}");
                    continue;
                }
                panic!("{encoder}: {message}");
            }
        };
        assert_eq!(rendered.format, format);
        let path = scratch.path().join(format!("alpha.{}", format.extension()));
        fs::write(&path, &rendered.bytes).unwrap();
        let input = InputContext::open(path.to_string_lossy().to_string()).unwrap();
        let stream = input.best_video_stream().unwrap();
        let codec = match format {
            OutputFormat::Mov => lumen_ffmpeg::VideoCodec::ProRes,
            _ => lumen_ffmpeg::VideoCodec::Vp9,
        };
        // WebM signals VP9's alpha layer in the container.
        assert_eq!((stream.codec, stream.has_alpha), (codec, true), "{encoder}");
        let mut audio = AudioRangeReader::open(path.to_string_lossy().to_string()).unwrap();
        assert!(
            audio.read(0, SAMPLE_RATE / 10).unwrap().samples > 0,
            "{encoder} audio"
        );

        let (frames, _) = decode(&rendered.bytes, scratch.path());
        assert_eq!(frames.len(), 3);
        let frame = &frames[0];
        assert!(frame.has_alpha, "{encoder} decoded without alpha");
        let mut worst = [0.0f64; 2];
        for (x, y, pixel) in png.enumerate_pixels() {
            let decoded = straight_rgba(frame, x, y);
            let [r, g, b, a] = pixel.0.map(f64::from);
            let alpha_error = (decoded[3] * 255.0 - a).abs();
            worst[1] = worst[1].max(alpha_error);
            assert!(
                alpha_error <= alpha_tolerance,
                "{encoder} alpha at ({x}, {y}): {} vs {a}",
                decoded[3] * 255.0
            );
            // Colour is undefined where alpha is 0, and too faint to measure below an eighth.
            if a < 32.0 {
                continue;
            }
            for (channel, expected) in [r, g, b].into_iter().enumerate() {
                let error = (decoded[channel] * 255.0 - expected).abs();
                worst[0] = worst[0].max(error);
                assert!(
                    error <= colour_tolerance,
                    "{encoder} channel {channel} at ({x}, {y}), alpha {a}: {} vs {expected}",
                    decoded[channel] * 255.0
                );
            }
        }
        eprintln!(
            "{encoder}: worst colour error {:.2}, alpha error {:.2} of 255",
            worst[0], worst[1]
        );
    }
}

#[test]
fn alpha_in_a_format_or_encoder_that_cannot_hold_it_is_refused() {
    let setup = Setup::new();
    let composition = transparent();
    for (encoder, format, expected) in [
        ("libx264", OutputFormat::Mp4, "can't hold alpha"),
        ("libx264", OutputFormat::Mov, "holds prores, not h264"),
        ("h264_vulkan", OutputFormat::Mov, "holds prores, not h264"),
        ("prores_ks", OutputFormat::Png, "can't hold alpha"),
    ] {
        let mut options = setup.options(RenderOutput::of(format));
        options.encoder =
            Some(VideoEncoderChoice::single(encoder, EncoderQuality::default()).with_alpha(true));
        if format == OutputFormat::Png {
            // An image keeps alpha whatever the encoder settings say.
            assert!(render_project(&composition, &options, &mut |_| {}).is_ok());
            continue;
        }
        if lumen_ffmpeg::encoder_info(encoder).is_err() {
            continue;
        }
        let Err(error) = render_project(&composition, &options, &mut |_| {}) else {
            panic!("{encoder} in {format:?} can't write alpha");
        };
        assert_eq!(error.code(), ErrorCode::InvalidOutput, "{encoder}");
        assert!(error.to_string().contains(expected), "{encoder}: {error}");
    }
    assert!(setup.nothing_left_behind());
}

#[test]
fn resolving_alpha_names_what_can_hold_it() {
    let quality = lumen_ffmpeg::QualityConfig::built_in();
    let alpha = |format, codec| {
        super::resolve_encoder(
            &quality,
            &transparent(),
            format,
            lumen_ffmpeg::QualityRequest {
                codec,
                alpha: Some(true),
                ..lumen_ffmpeg::QualityRequest::default()
            },
        )
    };
    let mp4 = alpha(OutputFormat::Mp4, None).unwrap_err();
    assert_eq!(mp4.code(), ErrorCode::InvalidOutput);
    assert!(
        mp4.to_string().contains("mov (prores) or webm (vp9)"),
        "{mp4}"
    );
    let h264 = alpha(OutputFormat::Mov, Some(lumen_ffmpeg::EncodeCodec::H264)).unwrap_err();
    assert!(h264.to_string().contains("a .mov holds prores"), "{h264}");
    let prores = alpha(OutputFormat::Mov, None).unwrap();
    assert_eq!(prores.first().encoder, "prores_ks");
    assert!(prores.alpha());
    assert_eq!(prores.bit_depth(), lumen_ffmpeg::BitDepth::Ten);
    // The composition's render_settings ask for alpha when the request doesn't.
    let asks = parse_composition(&json!({
        "timeline": { "fps": FPS, "duration_frames": 1 },
        "render_settings": { "width": 16, "height": 16, "alpha": true, "codec": "prores" },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 0] } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    }))
    .unwrap();
    let from_settings = super::resolve_encoder(
        &quality,
        &asks,
        OutputFormat::Mov,
        lumen_ffmpeg::QualityRequest::default(),
    )
    .unwrap();
    assert_eq!(
        (
            from_settings.first().encoder.as_str(),
            from_settings.alpha()
        ),
        ("prores_ks", true)
    );
    let opaque = super::resolve_encoder(
        &quality,
        &asks,
        OutputFormat::Mov,
        lumen_ffmpeg::QualityRequest {
            alpha: Some(false),
            ..lumen_ffmpeg::QualityRequest::default()
        },
    )
    .unwrap();
    assert!(!opaque.alpha());
}
