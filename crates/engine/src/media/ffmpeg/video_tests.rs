use std::{fs, path::PathBuf, process::Command};

use super::*;
use crate::media::YuvChroma;

const NTSC_RATE: &str = "30000/1001";

/// Encodes frames whose luma is `16 + 3 * n`, so a decoded frame identifies its own index.
fn encode_counting_clip(name: &str, args: &[&str]) -> Option<PathBuf> {
    let source =
        format!("nullsrc=s=32x32:r={NTSC_RATE}:d=2,geq=lum='16+3*N':cb=128:cr=128,format=yuv420p");
    encode_clip(name, &source, args)
}

fn encode_clip(name: &str, source: &str, args: &[&str]) -> Option<PathBuf> {
    let path = std::env::temp_dir().join(format!("lumen_engine_{}_{name}", std::process::id()));
    let output = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", source])
        .args(args)
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-qp",
            "1",
            "-bf",
            "0",
        ])
        .arg(&path)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("ffmpeg CLI not installed; skipping {name}");
            return None;
        }
        Err(error) => panic!("{name}: failed to run ffmpeg: {error}"),
    };
    if output.status.success() {
        return Some(path);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("Unknown encoder") || stderr.contains("Encoder not found") {
        eprintln!(
            "ffmpeg encoder unavailable; skipping {name}: {}",
            stderr.trim()
        );
        return None;
    }
    panic!(
        "{name}: ffmpeg fixture failed ({}): {stderr}",
        output.status
    );
}

fn open_cpu(path: &std::path::Path) -> Result<FfmpegVideoResolver, MediaError> {
    FfmpegVideoResolver::open_with_options(
        path.to_string_lossy().to_string(),
        FfmpegResolverOptions::default(),
    )
}

fn decoded_frame_index(resolver: &FfmpegVideoResolver, frame: u32) -> u32 {
    frame_index(&resolver.frame(frame).expect("decode frame"))
}

fn yuv_frame(frame: MediaFrame) -> Arc<YuvFrame> {
    let MediaFrame::Yuv(frame) = frame else {
        panic!("expected a YUV frame");
    };
    frame
}

fn frame_index(frame: &MediaFrame) -> u32 {
    let MediaFrame::Yuv(decoded) = frame else {
        panic!("expected a YUV frame");
    };
    // The first luma sample is `16 + 3 * index`.
    (f64::from(decoded.data[0] - 16) / 3.0).round() as u32
}

/// The alpha at a frame's first pixel, from 0 to 1, or `None` if it has no alpha plane.
fn first_alpha(frame: &YuvFrame) -> Option<f32> {
    let plane = frame.layout.alpha?;
    let depth = frame.format.depth;
    let sample = if depth > 8 {
        u16::from_le_bytes([frame.data[plane.offset], frame.data[plane.offset + 1]])
    } else {
        u16::from(frame.data[plane.offset])
    };
    Some(f32::from(sample) / ((1u32 << depth) - 1) as f32)
}

#[test]
fn frame_indexes_count_from_the_stream_start_time() {
    // MPEG-TS seeks land between keyframes, so that case only reads forward.
    for (name, frames) in [
        ("offset.mp4", &[0, 5, 40, 12][..]),
        ("offset.ts", &[0, 5, 40][..]),
    ] {
        let Some(path) = encode_counting_clip(name, &["-output_ts_offset", "1.4"]) else {
            return;
        };
        let resolver = open_cpu(&path).expect("open clip");
        for frame in frames {
            assert_eq!(decoded_frame_index(&resolver, *frame), *frame, "{name}");
        }
        let _ = fs::remove_file(path);
    }
}

#[test]
fn frame_count_falls_back_to_the_container_duration() {
    // Matroska stores no per-stream duration or frame count here, and its container duration
    // includes the start offset.
    for (name, args) in [
        ("no_stream_duration.mkv", &[][..]),
        (
            "offset_no_stream_duration.mkv",
            &["-output_ts_offset", "1.4"][..],
        ),
    ] {
        let Some(path) = encode_counting_clip(name, args) else {
            return;
        };
        let resolver = open_cpu(&path).expect("open clip");
        let metadata = resolver.metadata();
        assert_eq!(metadata.frame_count, 60, "{name}");
        assert!((f64::from(metadata.fps) - 30_000.0 / 1001.0).abs() < 0.001);
        assert_eq!(decoded_frame_index(&resolver, 59), 59, "{name}");
        let _ = fs::remove_file(path);
    }
}

#[test]
fn audio_preroll_does_not_add_frames_to_the_container_duration() {
    // AAC priming puts the Matroska start before zero, ahead of a video stream at zero.
    let Some(path) = encode_clip(
        "audio_preroll.mkv",
        "testsrc2=s=32x32:r=20:d=2",
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=d=2",
            "-c:a",
            "aac",
            "-avoid_negative_ts",
            "disabled",
        ],
    ) else {
        return;
    };
    let resolver = open_cpu(&path).expect("open clip");
    assert_eq!(resolver.metadata().frame_count, 40);
    assert!(resolver.frame(39).is_ok());
    let _ = fs::remove_file(path);
}

#[test]
fn a_stream_copied_cut_counts_only_the_frames_that_play() {
    // The cut starts on a keyframe, and an edit list hides its 15 preroll frames.
    let dir = std::env::temp_dir().join(format!("lumen_engine_{}_cut", std::process::id()));
    fs::create_dir_all(&dir).expect("create fixture dir");
    let full = dir.join("full.mp4");
    let cut = dir.join("cut.mp4");
    let run = |args: &[&str]| {
        let status = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(args)
            .status();
        match status {
            Ok(status) => {
                assert!(status.success(), "ffmpeg {args:?}");
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("ffmpeg CLI not installed; skipping the cut clip");
                false
            }
            Err(error) => panic!("failed to run ffmpeg: {error}"),
        }
    };
    let encoded = run(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=160x90:rate=30:duration=10",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-bf",
        "2",
        "-g",
        "24",
        "-keyint_min",
        "24",
        "-sc_threshold",
        "0",
        "-pix_fmt",
        "yuv420p",
        full.to_str().expect("utf-8 path"),
    ]);
    if !encoded {
        return;
    }
    assert!(run(&[
        "-ss",
        "2.1",
        "-i",
        full.to_str().expect("utf-8 path"),
        "-t",
        "8",
        "-c",
        "copy",
        cut.to_str().expect("utf-8 path"),
    ]));

    let full_clip = open_cpu(&full).expect("open full clip");
    let cut_clip = open_cpu(&cut).expect("open cut clip");
    assert_eq!(full_clip.metadata().frame_count, 300);
    assert_eq!(cut_clip.metadata().frame_count, 237);
    let last = yuv_frame(cut_clip.frame(236).expect("decode the last frame"));
    let source_last = yuv_frame(
        full_clip
            .frame(299)
            .expect("decode the source's last frame"),
    );
    assert_eq!(last.data, source_last.data);
    assert!(cut_clip.frame(237).is_err());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn images_read_as_video_count_their_own_frames() {
    const ONE_PIXEL_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xef, 0xa0, 0xf0, 0x1f, 0x00, 0x06, 0x00, 0x02, 0x5f, 0x34, 0xcd, 0x40, 0xc0, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let still = std::env::temp_dir().join(format!("lumen_engine_{}_still.png", std::process::id()));
    fs::write(&still, ONE_PIXEL_PNG).expect("write png");
    let resolver = open_cpu(&still).expect("open png");
    assert_eq!(resolver.metadata().frame_count, 1);
    assert!(resolver.frame(0).is_ok());
    let _ = fs::remove_file(still);

    let animated =
        std::env::temp_dir().join(format!("lumen_engine_{}_animated.apng", std::process::id()));
    let Ok(generated) = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "testsrc2=s=32x32:r=25:d=0.4", "-f", "apng"])
        .arg(&animated)
        .status()
    else {
        eprintln!("ffmpeg CLI unavailable; skipping APNG frame count");
        return;
    };
    assert!(generated.success(), "generate APNG");
    let resolver = open_cpu(&animated).expect("open apng");
    assert_eq!(resolver.metadata().frame_count, 10);
    assert!(resolver.frame(9).is_ok());
    let _ = fs::remove_file(animated);
}

#[test]
fn a_missing_file_fails_to_open() {
    let error = open_cpu(&std::env::temp_dir().join("lumen_engine_no_such_clip.mp4"))
        .err()
        .expect("no such file");
    assert_eq!(error.code(), "media.not_found", "{error}");
}

#[test]
fn missing_duration_is_an_error_instead_of_a_one_frame_clip() {
    let Some(path) = encode_counting_clip("elementary.h264", &["-f", "h264"]) else {
        return;
    };
    let error = open_cpu(&path)
        .err()
        .expect("no duration to count frames from");
    assert!(error.to_string().contains("frame count"), "{error}");
    let _ = fs::remove_file(path);
}

#[test]
fn a_failing_hardware_decoder_moves_the_source_to_the_cpu_for_good() {
    let Some(path) = encode_counting_clip("hardware_failure.mp4", &[]) else {
        return;
    };
    // A CPU decoder marked active as hardware refuses `receive_gpu_frame` on the first frame,
    // the state a hardware decoder that opens and then can't decode is in, on any machine.
    let mut decoder =
        LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
            .expect("open clip");
    decoder.options.prefer_hardware_decode = true;
    decoder.gpu_decode_status = GpuDecodeStatus::Active {
        backend: GpuBackend::Vulkan,
    };

    for frame in [5, 6, 2] {
        let decoded = decoder
            .decode_frame(frame, &mut FrameLruCache::default())
            .expect("decode on the CPU");
        assert_eq!(frame_index(&decoded), frame);
        assert!(!decoder.options.prefer_hardware_decode);
        assert!(!matches!(
            decoder.gpu_decode_status,
            GpuDecodeStatus::Active { .. }
        ));
    }
    decoder.reopen().expect("reopen");
    assert!(!matches!(
        decoder.gpu_decode_status,
        GpuDecodeStatus::Active { .. }
    ));
    let _ = fs::remove_file(path);
}

#[test]
fn a_failed_hardware_decoder_is_released_before_its_replacement_opens() {
    let Some(path) = encode_counting_clip("release_first.mp4", &[]) else {
        return;
    };
    let source = path.to_string_lossy().to_string();
    let mut decoder =
        LibavStreamDecoder::open(source.clone(), Default::default()).expect("open clip");
    let cached = decoder
        .decode_frame(2, &mut FrameLruCache::default())
        .expect("frame to retire");
    decoder.options.prefer_hardware_decode = true;
    decoder.gpu_decode_status = GpuDecodeStatus::Active {
        backend: GpuBackend::Vulkan,
    };
    let mut worker = VideoDecodeWorker::new(decoder);
    worker.cache.insert(2, cached, false);

    // A source that can't reopen stands in for one out of file descriptors.
    worker.decoder.source = format!("{source}.missing");
    worker.resolve_frame(3).unwrap_err();
    assert!(worker.decoder.decoder.is_none());
    assert!(!worker.cache.contains(2));
    worker.resolve_frame(3).unwrap_err();

    worker.decoder.source = source;
    let decoded = worker
        .resolve_frame(3)
        .expect("the next decode tries again");
    assert_eq!(frame_index(&decoded), 3);
    let _ = fs::remove_file(path);
}

#[test]
fn a_frame_the_renderer_rejects_moves_the_source_to_the_cpu_and_drops_cached_gpu_frames() {
    let Some(path) = encode_counting_clip("import_failure.mp4", &[]) else {
        return;
    };
    let mut decoder =
        LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
            .expect("open clip");
    let cached = decoder
        .decode_frame(8, &mut FrameLruCache::default())
        .expect("frame to retire");
    decoder.gpu_decode_status = GpuDecodeStatus::Active {
        backend: GpuBackend::Vulkan,
    };
    decoder.options.prefer_hardware_decode = true;
    let mut worker = VideoDecodeWorker::new(decoder);
    worker.cache.insert(8, cached, false);

    let retried = worker.resolve_on_cpu(9, "import failed").expect("retry");
    assert_eq!(frame_index(&retried), 9);
    assert!(!worker.decoder.options.prefer_hardware_decode);
    assert!(matches!(
        worker.decoder.gpu_decode_status,
        GpuDecodeStatus::NotRequested
    ));
    assert!(!worker.cache.contains(8));

    let again = worker.resolve_on_cpu(3, "still failing").expect("retry");
    assert_eq!(frame_index(&again), 3);
    // A source already on the CPU may still have cached a GPU frame before it moved.
    assert!(!worker.cache.contains(9) && worker.cache.contains(3));
    let _ = fs::remove_file(path);
}

/// A CPU decoder marked active as hardware, whose next frame fails like a hardware decoder's.
fn failing_hardware_decoder(path: &std::path::Path) -> LibavStreamDecoder {
    let mut decoder =
        LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
            .expect("open clip");
    decoder.options.prefer_hardware_decode = true;
    decoder.gpu_decode_status = GpuDecodeStatus::Active {
        backend: GpuBackend::Vulkan,
    };
    decoder
}

#[test]
fn a_source_settled_on_the_gpu_keeps_failing_across_a_reopen() {
    let Some(path) = encode_counting_clip("reopen_settled.mp4", &[]) else {
        return;
    };
    let mut decoder = failing_hardware_decoder(&path);
    let (request_tx, _request_rx) = mpsc::channel();
    decoder.worker = Some(request_tx);
    decoder.origin.set(PixelOrigin::Gpu).ok();
    let origin = Arc::clone(&decoder.origin);
    let mut cache = FrameLruCache::default();

    let failure = decoder.decode_frame(5, &mut cache).unwrap_err().to_string();
    assert!(failure.contains("would change"), "{failure}");

    // A reopen that finds the hardware decoder gone fails the same way.
    let gone = decoder.reopen().unwrap_err().to_string();
    assert!(gone.contains("would change"), "{gone}");

    decoder.options.prefer_hardware_decode = false;
    decoder.reopen().expect("reopen");
    assert!(Arc::ptr_eq(&origin, &decoder.origin));
    assert!(decoder.worker.is_some());
    for frame in [9, 2] {
        let later = decoder.decode_frame(frame, &mut cache).unwrap_err();
        assert_eq!(later.to_string(), gone);
    }
    let _ = fs::remove_file(path);
}

#[test]
fn seeks_after_a_reopen_decode_the_right_frames_and_leave_the_source_unsettled() {
    let Some(path) = encode_counting_clip("reopen_seeks.mp4", &[]) else {
        return;
    };
    let mut decoder =
        LibavStreamDecoder::open(path.to_string_lossy().to_string(), Default::default())
            .expect("open clip");
    let (request_tx, _request_rx) = mpsc::channel();
    decoder.worker = Some(request_tx);
    let origin = Arc::clone(&decoder.origin);
    let mut cache = FrameLruCache::default();

    for frame in [6, 7] {
        let decoded = decoder.decode_frame(frame, &mut cache).expect("decode");
        assert_eq!(frame_index(&decoded), frame);
    }
    decoder.reopen().expect("reopen");
    assert!(Arc::ptr_eq(&origin, &decoder.origin));
    assert!(decoder.worker.is_some());
    for frame in [12, 3, 4, 40, 1] {
        let decoded = decoder.decode_frame(frame, &mut cache).expect("decode");
        assert_eq!(frame_index(&decoded), frame);
    }
    assert!(origin.get().is_none());
    let _ = fs::remove_file(path);
}

/// The fixture is an open GOP, with CRA keyframes every 12 frames and a RASL frame before each:
/// `ffmpeg -f lavfi -i testsrc2=s=128x72:r=30:d=3 -g 12 -c:v libx265 -preset veryfast -crf 24
/// -tag:v hvc1 -pix_fmt yuv420p open_gop.mp4`.
#[test]
fn stepping_back_before_an_open_gop_keyframe_decodes_the_frame_asked_for() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/open_gop.mp4");
    let mut cache = FrameLruCache::default();
    let mut stepping = LibavStreamDecoder::open(path, Default::default()).expect("open clip");
    // Frames 59 and 35 are RASL frames, and a seek to 57 also lands on CRA 60.
    for frame in [60, 59, 36, 35, 60, 57] {
        let stepped = yuv_frame(stepping.decode_frame(frame, &mut cache).expect("decode"));
        let fresh = LibavStreamDecoder::open(path, Default::default())
            .expect("open clip")
            .decode_frame(frame, &mut cache)
            .map(yuv_frame)
            .expect("decode");
        assert!(stepped.data == fresh.data, "frame {frame}");
    }
}

/// Runs on a GPU with Vulkan decode for `libx264` or a file in `LUMEN_VULKAN_DECODE_FILES`.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
#[test]
fn a_hardware_decoder_stays_on_the_gpu_through_a_reopen_and_seeks() {
    // Held to the end, so that the instance this test makes and drops overlaps no other test's.
    let _shared = crate::gpu::test_device::shared();
    let Ok((_renderer, device)) =
        pollster::block_on(crate::gpu::GpuCompositionRenderer::new_vulkan_decode())
    else {
        return;
    };
    let mut options = FfmpegResolverOptions::with_hardware_decode(true);
    options.vulkan_device = Some(device);
    let mut clips: Vec<_> = std::env::var_os("LUMEN_VULKAN_DECODE_FILES")
        .map(|files| std::env::split_paths(&files).collect())
        .unwrap_or_default();
    clips.extend(encode_clip(
        "reopen_gpu.mp4",
        "testsrc2=s=320x240:r=30:d=2",
        &["-pix_fmt", "yuv420p"],
    ));
    let mut cache = FrameLruCache::default();
    for clip in &clips {
        let source = clip.to_string_lossy().to_string();
        let Ok(mut decoder) = LibavStreamDecoder::open(source.clone(), options.clone()) else {
            continue;
        };
        let on_gpu = |decoded: &MediaFrame| matches!(decoded, MediaFrame::GpuVideo(_));
        if !decoder
            .decode_frame(6, &mut cache)
            .is_ok_and(|d| on_gpu(&d))
        {
            eprintln!("{source} doesn't decode on this hardware; skipping");
            continue;
        }
        let Ok(MediaFrame::GpuVideo(drawn)) = decoder.decode_frame(7, &mut cache) else {
            panic!("{source}: frame 7 should decode on the GPU like frame 6");
        };
        assert!(drawn.settle_on_gpu());

        decoder.reopen().expect("reopen");
        for frame in [20, 3, 4, 30] {
            let Ok(MediaFrame::GpuVideo(decoded)) = decoder.decode_frame(frame, &mut cache) else {
                panic!("{source}: frame {frame} left the GPU after a reopen");
            };
            assert!(decoded.settle_on_gpu());
            assert!(!decoder.settle_on_cpu());
            assert_eq!(decoder.frame_for_pts(decoded.frame.pts()), frame);
        }
    }
}

#[test]
fn sources_with_alpha_warn_skip_hardware_decode_and_keep_their_alpha() {
    let path = std::env::temp_dir().join(format!("lumen_engine_{}_alpha.mov", std::process::id()));
    let Ok(output) = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args([
            "-i",
            "color=c=0x2080ff:s=32x32:r=30:d=0.2,format=rgba,colorchannelmixer=aa=0.5,format=yuva444p10le",
        ])
        .args(["-c:v", "prores_ks", "-profile:v", "4444", "-alpha_bits", "16"])
        .arg(&path)
        .output()
    else {
        eprintln!("ffmpeg CLI not installed; skipping alpha routing test");
        return;
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Unknown encoder"), "{stderr}");
        eprintln!("prores_ks unavailable; skipping alpha routing test");
        return;
    }

    let resolver = FfmpegVideoResolver::open_with_options(
        path.to_string_lossy().to_string(),
        FfmpegResolverOptions::with_hardware_decode(true),
    )
    .expect("open clip");
    let reason = resolver.decode_unavailable_reason().expect("a warning");
    assert!(reason.contains("alpha"), "{reason}");
    let decoded = yuv_frame(resolver.frame(0).expect("decode frame"));
    assert_eq!(decoded.format.depth, 12);
    let alpha = first_alpha(&decoded).expect("an alpha plane");
    assert!((alpha - 0.5).abs() <= 0.01, "{alpha}");
    let _ = fs::remove_file(path);
}

#[test]
fn vp9_alpha_layers_are_detected_and_decoded_into_an_alpha_plane() {
    let path = std::env::temp_dir().join(format!("lumen_engine_{}_alpha.webm", std::process::id()));
    let Ok(output) = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args([
            "-i",
            "color=c=0x2080ff:s=32x32:r=30:d=0.2,format=rgba,colorchannelmixer=aa=0.5,format=yuva420p",
        ])
        .args(["-c:v", "libvpx-vp9", "-crf", "10", "-b:v", "0"])
        .arg(&path)
        .output()
    else {
        eprintln!("ffmpeg CLI not installed; skipping VP9 alpha test");
        return;
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Unknown encoder"), "{stderr}");
        eprintln!("libvpx-vp9 unavailable; skipping VP9 alpha test");
        return;
    }

    let source = path.to_string_lossy().to_string();
    let input = open_input(&source).expect("open clip");
    assert!(input.best_video_stream().expect("video stream").has_alpha);
    let resolver = FfmpegVideoResolver::open_with_options(source, FfmpegResolverOptions::default())
        .expect("open clip");
    let decoded = yuv_frame(resolver.frame(0).expect("decode frame"));
    let alpha = first_alpha(&decoded).expect("an alpha plane");
    assert!((alpha - 0.5).abs() <= 0.01, "{alpha}");
    let _ = fs::remove_file(path);
}

/// The stream's parameters come from its opaque start, so only the frames show the alpha.
#[test]
fn frames_that_gain_alpha_mid_stream_carry_an_alpha_plane() {
    let directory =
        std::env::temp_dir().join(format!("lumen_engine_{}_gains_alpha", std::process::id()));
    fs::create_dir_all(&directory).expect("create directory");
    let encode = |name: &str, filter: &str, pixel_format: &str| {
        let path = directory.join(name);
        Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args(["-i", filter])
            .args(["-pix_fmt", pixel_format, "-c:v", "prores_ks"])
            .args(["-profile:v", "4444", "-alpha_bits", "16"])
            .arg(&path)
            .output()
            .map(|output| (path, output))
    };
    let parts = [
        encode(
            "opaque.mov",
            "color=c=0x2080ff:s=32x32:r=30:d=0.1",
            "yuv444p10le",
        ),
        encode(
            "alpha.mov",
            "color=c=0x2080ff:s=32x32:r=30:d=0.1,format=rgba,colorchannelmixer=aa=0.5",
            "yuva444p10le",
        ),
    ];
    let mut list = String::new();
    for part in &parts {
        let Ok((path, output)) = part else {
            eprintln!("ffmpeg CLI not installed; skipping mid-stream alpha test");
            return;
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("Unknown encoder"), "{stderr}");
            eprintln!("prores_ks unavailable; skipping mid-stream alpha test");
            return;
        }
        list.push_str(&format!("file '{}'\n", path.display()));
    }
    let list_path = directory.join("list.txt");
    fs::write(&list_path, list).expect("write concat list");
    let path = directory.join("gains_alpha.mov");
    let output = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "concat", "-safe", "0", "-i"])
        .arg(&list_path)
        .args(["-c", "copy"])
        .arg(&path)
        .output()
        .expect("concat clips");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let source = path.to_string_lossy().to_string();
    let input = open_input(&source).expect("open clip");
    assert!(!input.best_video_stream().expect("video stream").has_alpha);
    let resolver = FfmpegVideoResolver::open_with_options(source, FfmpegResolverOptions::default())
        .expect("open clip");
    let opaque = yuv_frame(resolver.frame(0).expect("decode frame"));
    assert!(first_alpha(&opaque).is_none());
    let translucent = yuv_frame(resolver.frame(4).expect("decode frame"));
    let alpha = first_alpha(&translucent).expect("an alpha plane");
    assert!((alpha - 0.5).abs() <= 0.01, "{alpha}");
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn yuv_frames_carry_the_stream_tags_and_the_untagged_rule() {
    struct Clip {
        name: &'static str,
        source: &'static str,
        args: &'static [&'static str],
        /// Clockwise degrees to rotate the file's display by, through a remux.
        display_rotation: Option<&'static str>,
        matrix: YuvMatrix,
        full_range: bool,
        rotation: Rotation,
    }
    const SD: &str = "testsrc2=s=64x48:r=30:d=0.2";
    let clips = [
        Clip {
            name: "tagged_709.mp4",
            source: SD,
            args: &[
                "-pix_fmt",
                "yuv420p",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
            ],
            display_rotation: None,
            matrix: YuvMatrix::Bt709,
            full_range: false,
            rotation: Rotation::None,
        },
        Clip {
            name: "tagged_2020_full.mp4",
            source: SD,
            args: &[
                "-pix_fmt",
                "yuv420p",
                "-colorspace",
                "bt2020nc",
                "-color_range",
                "pc",
            ],
            display_rotation: None,
            matrix: YuvMatrix::Bt2020,
            full_range: true,
            rotation: Rotation::None,
        },
        Clip {
            name: "tagged_601_hd.mp4",
            source: "testsrc2=s=1280x720:r=30:d=0.1",
            args: &["-pix_fmt", "yuv420p", "-colorspace", "smpte170m"],
            display_rotation: None,
            matrix: YuvMatrix::Bt601,
            full_range: false,
            rotation: Rotation::None,
        },
        Clip {
            name: "untagged_sd.mp4",
            source: SD,
            args: &["-pix_fmt", "yuv420p"],
            display_rotation: None,
            matrix: YuvMatrix::Bt601,
            full_range: false,
            rotation: Rotation::None,
        },
        Clip {
            name: "untagged_hd.mp4",
            source: "testsrc2=s=1280x720:r=30:d=0.1",
            args: &["-pix_fmt", "yuv420p"],
            display_rotation: None,
            matrix: YuvMatrix::Bt709,
            full_range: false,
            rotation: Rotation::None,
        },
        Clip {
            name: "jpeg_range.mp4",
            source: SD,
            args: &["-pix_fmt", "yuvj420p"],
            display_rotation: None,
            matrix: YuvMatrix::Bt601,
            full_range: true,
            rotation: Rotation::None,
        },
        Clip {
            name: "rotated.mp4",
            source: SD,
            args: &["-pix_fmt", "yuv420p"],
            // FFmpeg's `display_rotation` is counterclockwise.
            display_rotation: Some("90"),
            matrix: YuvMatrix::Bt601,
            full_range: false,
            rotation: Rotation::Cw270,
        },
    ];
    for clip in clips {
        let Some(mut path) = encode_clip(clip.name, clip.source, clip.args) else {
            return;
        };
        if let Some(degrees) = clip.display_rotation {
            let rotated = path.with_extension("rotated.mp4");
            let status = Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error"])
                .args(["-display_rotation", degrees, "-i"])
                .arg(&path)
                .args(["-c", "copy"])
                .arg(&rotated)
                .status()
                .expect("run ffmpeg");
            assert!(status.success(), "{}: remux", clip.name);
            let _ = fs::remove_file(&path);
            path = rotated;
        }
        let resolver = open_cpu(&path).expect("open clip");
        let decoded = yuv_frame(resolver.frame(0).expect("decode frame"));
        let format = decoded.format;
        let name = clip.name;
        assert_eq!(format.color.matrix, clip.matrix, "{name}");
        assert_eq!(format.color.full_range, clip.full_range, "{name}");
        assert_eq!(format.rotation, clip.rotation, "{name}");
        assert_eq!(format.subsampling, ChromaSubsampling::Yuv420, "{name}");
        assert_eq!((format.depth, format.msb_aligned), (8, false), "{name}");
        assert!(matches!(decoded.layout.chroma, YuvChroma::Planar { .. }));
        assert!(decoded.layout.alpha.is_none(), "{name}");
        // The frame keeps its coded size and the stream's size is the upright one.
        let metadata = resolver.metadata();
        assert_eq!(
            format.display_size(),
            (metadata.width, metadata.height),
            "{name}"
        );
        let _ = fs::remove_file(path);
    }
}

#[test]
fn hdr_and_wide_gamut_tags_reach_the_frame_and_unconverted_ones_warn() {
    const SD: &str = "testsrc2=s=64x48:r=30:d=0.2";
    let cases = [
        (
            "pq.mp4",
            "color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc",
            ColorPrimaries::Bt2020,
            TransferFunction::Pq,
            None,
        ),
        (
            "hlg_p3.mp4",
            "color_primaries=smpte432:color_trc=arib-std-b67:colorspace=bt709",
            ColorPrimaries::DisplayP3,
            TransferFunction::Hlg,
            None,
        ),
        (
            "log.mp4",
            "color_primaries=film:color_trc=log100:colorspace=bt709",
            ColorPrimaries::Bt709,
            TransferFunction::Sdr,
            Some(
                "its `log100` transfer is read as SDR and its `film` primaries are read as \
                     BT.709, which the renderer doesn't convert",
            ),
        ),
        (
            "untagged.mp4",
            "colorspace=bt709",
            ColorPrimaries::Bt709,
            TransferFunction::Sdr,
            None,
        ),
    ];
    for (name, params, primaries, transfer, warning) in cases {
        let Some(path) = encode_clip(name, SD, &["-vf", &format!("setparams={params}")]) else {
            return;
        };
        let resolver = open_cpu(&path).expect("open clip");
        assert_eq!(resolver.color_warning(), warning, "{name}");
        let color = yuv_frame(resolver.frame(0).expect("decode frame"))
            .format
            .color;
        assert_eq!(
            (color.primaries, color.transfer),
            (primaries, transfer),
            "{name}"
        );
        let _ = fs::remove_file(path);
    }
}

#[test]
fn hdr_video_that_is_not_yuv_warns_that_it_is_read_as_srgb() {
    use lumen_ffmpeg::{ColorMatrix, ColorTags, ColorTransfer};
    let sdr = ColorTags::new(ColorMatrix::Other, true);
    let pq = ColorTags {
        transfer: ColorTransfer::Smpte2084,
        ..sdr
    };
    assert_eq!(unsupported_color(sdr, false), None);
    assert_eq!(unsupported_color(pq, true), None);
    let warning = unsupported_color(pq, false).expect("a warning");
    assert!(warning.contains("read as 8-bit sRGB"), "{warning}");
}

#[test]
fn constant_luminance_sources_are_rejected_when_opened() {
    let Some(path) = encode_clip(
        "constant_luminance.mp4",
        "testsrc2=s=64x48:r=30:d=0.2",
        &["-vf", "setparams=colorspace=bt2020c"],
    ) else {
        return;
    };
    let Err(MediaError::Failed(failure)) = open_cpu(&path) else {
        panic!("a constant-luminance source opens");
    };
    assert_eq!(failure.reason.code(), "media.unsupported");
    assert!(failure.to_string().contains("bt2020nc"), "{failure}");
    let _ = fs::remove_file(path);
}

#[test]
fn unusual_yuv_sources_reshape_without_changing_luma_and_rgb_sources_stay_rgba() {
    let encode = |name: &str, pixel_format: &str| {
        let path = std::env::temp_dir().join(format!("lumen_engine_{}_{name}", std::process::id()));
        let output = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args([
                "-i",
                "nullsrc=s=32x32:r=30:d=0.5,geq=lum='16+3*N':cb=128:cr=128,format=yuv420p",
            ])
            .args(["-pix_fmt", pixel_format, "-c:v", "ffv1"])
            .arg(&path)
            .output();
        match output {
            Ok(output) if output.status.success() => Some(path),
            _ => {
                eprintln!("ffmpeg CLI with ffv1 unavailable; skipping {name}");
                None
            }
        }
    };
    let (Some(yuv411), Some(rgb)) = (encode("yuv411.mkv", "yuv411p"), encode("rgb.mkv", "bgr0"))
    else {
        return;
    };

    let reshaped = open_cpu(&yuv411)
        .expect("open clip")
        .frame(5)
        .expect("decode");
    assert_eq!(frame_index(&reshaped), 5);
    let reshaped = yuv_frame(reshaped);
    assert_eq!(reshaped.format.subsampling, ChromaSubsampling::Yuv422);
    assert_eq!(reshaped.data.len(), 32 * 32 * 2);
    reshaped.validate().expect("planes fit the buffer");

    let rgb_frame = open_cpu(&rgb).expect("open clip").frame(5).expect("decode");
    assert!(matches!(rgb_frame, MediaFrame::CpuRgba(_)));
    let _ = fs::remove_file(yuv411);
    let _ = fs::remove_file(rgb);
}

#[test]
fn opaque_sources_report_no_alpha() {
    let Some(path) = encode_counting_clip("opaque.mp4", &[]) else {
        return;
    };
    let input = open_input(&path.to_string_lossy()).expect("open clip");
    assert!(!input.best_video_stream().expect("video stream").has_alpha);
    let _ = fs::remove_file(path);
}
