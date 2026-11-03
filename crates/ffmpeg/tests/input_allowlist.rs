//! `InputContext::open` must keep reading what people upload and refuse inputs that name other files.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use lumen_ffmpeg::{
    ALLOWED_INPUT_FORMATS, CpuVideoFrame, InputContext, MuxedEncoder, PixelFormat, VideoCodec,
    VideoEncoderConfig,
};

const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xef, 0xa0, 0xf0,
    0x1f, 0x00, 0x06, 0x00, 0x02, 0x5f, 0x34, 0xcd, 0x40, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A grayscale baseline JPEG of flat blocks. At 1280x720 its end marker sits past the 2 KB FFmpeg
/// probes first, which is what makes the probe pick `image2` over `jpeg_pipe` for a `.jpg`.
fn flat_jpeg(width: u16, height: u16) -> Vec<u8> {
    let blocks = usize::from(width.div_ceil(8)) * usize::from(height.div_ceil(8));
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xdb, 0x00, 0x43, 0x00];
    jpeg.extend([1; 64]);
    jpeg.extend([0xff, 0xc0, 0x00, 0x0b, 0x08]);
    jpeg.extend(height.to_be_bytes());
    jpeg.extend(width.to_be_bytes());
    jpeg.extend([0x01, 0x01, 0x11, 0x00]);
    // DC and AC tables holding one 1-bit code, for symbol 0.
    for class in [0x00, 0x10] {
        jpeg.extend([0xff, 0xc4, 0x00, 0x14, class, 0x01]);
        jpeg.extend([0; 16]);
    }
    jpeg.extend([0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3f, 0x00]);
    // Each block is "no DC change" and "end of block": two zero bits.
    jpeg.resize(jpeg.len() + (blocks * 2).div_ceil(8), 0);
    jpeg.extend([0xff, 0xd9]);
    jpeg
}

/// A 16x16 RGB PCX. The non-zero filler byte drops `pcx_pipe`'s probe score below `image2`'s, which
/// is what lets a renamed PCX reach `image2`.
fn low_score_pcx() -> Vec<u8> {
    let mut pcx = vec![0; 128];
    pcx[..4].copy_from_slice(&[10, 5, 1, 8]);
    pcx[8] = 15;
    pcx[10] = 15;
    pcx[65] = 3;
    pcx[66] = 16;
    pcx[74] = 1;
    pcx.resize(128 + 16 * 3 * 16, 0x40);
    pcx
}

fn video_size(path: &Path) -> (u32, u32) {
    let input = InputContext::open(path.to_string_lossy().to_string())
        .unwrap_or_else(|error| panic!("{} must open: {error}", path.display()));
    let info = input.media_info();
    let stream = info.video_streams.first().expect("video stream");
    (stream.width, stream.height)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lumen_ffmpeg_{name}_{unique}"));
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn join(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_segment(path: &Path) -> bool {
    let config = VideoEncoderConfig::cpu_rgba(32, 24, 24, VideoCodec::H264);
    let Ok(mut encoder) = MuxedEncoder::create(path.to_string_lossy().to_string(), config) else {
        return false;
    };
    for pts in 0..6 {
        encoder
            .write_video_frame(&CpuVideoFrame {
                width: 32,
                height: 24,
                stride: 32 * 4,
                pixel_format: PixelFormat::Rgba8,
                pts: Some(pts),
                data: vec![128; 32 * 24 * 4],
            })
            .expect("write frame");
    }
    encoder.finish().expect("finish encode");
    true
}

fn assert_refused(path: &Path) {
    let error = InputContext::open(path.to_string_lossy().to_string())
        .expect_err(&format!("{} must not open", path.display()));
    assert!(
        error.to_string().contains("not supported"),
        "{}: {error}",
        path.display()
    );
}

#[test]
fn inputs_that_name_other_files_are_refused() {
    let dir = TempDir::new("references");
    let segment = dir.join("outside.ts");
    if !write_segment(&segment) {
        eprintln!("no H.264 encoder; skipping reference-following input test");
        return;
    }

    let playlist = dir.join("stream.m3u8");
    fs::write(
        &playlist,
        format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\n{}\n#EXT-X-ENDLIST\n",
            segment.display()
        ),
    )
    .expect("write playlist");
    assert_refused(&playlist);

    let concat = dir.join("list.ffconcat");
    fs::write(&concat, "ffconcat version 1.0\nfile outside.ts\n").expect("write concat list");
    assert_refused(&concat);

    fs::write(dir.join("frame1.png"), TINY_PNG).expect("write frame");
    assert_refused(&dir.join("frame%d.png"));
}

#[test]
fn image_paths_never_read_a_sibling_file() {
    let dir = TempDir::new("patterns");
    fs::write(dir.join("frame1.jpg"), flat_jpeg(1280, 720)).expect("write sibling");
    for pattern in ["frame%d.jpg", "frame*.jpg", "frame?.jpg", "frame{1,2}.jpg"] {
        assert_refused(&dir.join(pattern));
    }

    // A file really named like a pattern is that file, not a sequence of its neighbours.
    fs::write(dir.join("frame%d.jpg"), flat_jpeg(1280, 736)).expect("write literal");
    assert_eq!(video_size(&dir.join("frame%d.jpg")), (1280, 736));

    fs::write(dir.join("frame*.jpg"), flat_jpeg(1280, 752)).expect("write literal");
    assert_eq!(video_size(&dir.join("frame*.jpg")), (1280, 752));

    // `image2` opens `.U` and `.V` beside a `.y` file.
    for file in ["plane.y", "plane.U", "plane.V"] {
        fs::write(dir.join(file), vec![0; 4096]).expect("write plane");
    }
    assert_refused(&dir.join("plane.y"));
}

#[test]
fn renamed_images_cannot_reach_other_decoders_through_image2() {
    let dir = TempDir::new("renamed");
    for file in ["image.pcx", "image.jpg", "image.jpeg", "image.bmp"] {
        let path = dir.join(file);
        fs::write(&path, low_score_pcx()).expect("write pcx");
        assert_refused(&path);
    }
}

#[test]
fn normal_sized_jpegs_open_with_any_extension_case() {
    let dir = TempDir::new("jpeg");
    let jpeg = flat_jpeg(1280, 720);
    assert!(jpeg.len() > 2048, "must outgrow FFmpeg's first probe");
    for file in ["photo.jpg", "photo.jpeg", "PHOTO.JPG", "photo"] {
        let path = dir.join(file);
        fs::write(&path, &jpeg).expect("write jpeg");
        assert_eq!(video_size(&path), (1280, 720), "{file}");
    }
}

#[test]
fn allowlist_leaves_out_every_demuxer_that_opens_other_inputs() {
    let allowed: Vec<_> = ALLOWED_INPUT_FORMATS.split(',').collect();
    for demuxer in [
        "concat", "dash", "hls", "image2", "imf", "mlv", "rtsp", "sap", "sdp", "vobsub",
    ] {
        assert!(!allowed.contains(&demuxer), "{demuxer} must stay out");
    }
}

#[test]
fn images_open_without_needing_an_encoder() {
    let dir = TempDir::new("images");
    // A 1x1 lossless WebP.
    const TINY_WEBP: &[u8] = b"RIFF\x22\0\0\0WEBPVP8L\x15\0\0\0\x2f\0\0\0\0\x07\xd0\xff\xfe\xf7\xbf\xff\x81\x88\xe8\x7f\0\0";
    for (file, bytes) in [("image.png", TINY_PNG), ("image.webp", TINY_WEBP)] {
        let path = dir.join(file);
        fs::write(&path, bytes).expect("write image");
        InputContext::open(path.to_string_lossy().to_string())
            .unwrap_or_else(|error| panic!("{file} must open: {error}"));
    }
}

#[test]
fn common_upload_formats_still_open() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg CLI unavailable; skipping upload format test");
        return;
    }
    let dir = TempDir::new("uploads");
    let video = "testsrc2=size=32x24:rate=25:duration=0.3";
    let audio = "sine=frequency=440:duration=0.3";
    for (file, source, encoder_args) in [
        ("video.mp4", video, &["-c:v", "mpeg4"][..]),
        ("video.mov", video, &["-c:v", "mpeg4"]),
        ("video.mkv", video, &["-c:v", "mpeg4"]),
        ("video.ts", video, &["-c:v", "mpeg2video"]),
        ("video.mpg", video, &["-c:v", "mpeg2video"]),
        ("video.avi", video, &["-c:v", "mpeg4"]),
        ("video.flv", video, &["-c:v", "flv"]),
        ("video.wmv", video, &["-c:v", "mpeg4", "-f", "asf"]),
        ("video.mxf", video, &["-c:v", "mpeg2video"]),
        ("video.webm", video, &["-c:v", "libvpx"]),
        (
            "audio.webm",
            audio,
            &["-c:a", "opus", "-strict", "-2", "-ac", "2"],
        ),
        (
            "image.jpg",
            "testsrc2=size=1280x720:rate=25:duration=0.3",
            &["-frames:v", "1", "-pix_fmt", "yuvj420p"],
        ),
        ("image.bmp", video, &["-frames:v", "1"]),
        ("image.tiff", video, &["-frames:v", "1"]),
        ("image.gif", video, &[]),
        ("image.apng", video, &["-f", "apng"]),
        ("audio.wav", audio, &[]),
        ("audio.flac", audio, &[]),
        ("audio.aac", audio, &["-c:a", "aac"]),
        ("audio.m4a", audio, &["-c:a", "aac"]),
        ("audio.mp3", audio, &["-c:a", "libmp3lame"]),
        (
            "audio.ogg",
            audio,
            &["-c:a", "vorbis", "-strict", "-2", "-ac", "2"],
        ),
        ("audio.aiff", audio, &[]),
        ("audio.ac3", audio, &[]),
        ("audio.caf", audio, &[]),
    ] {
        let path = dir.join(file);
        let generated = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args(["-i", source])
            .args(encoder_args)
            .arg(&path)
            .status()
            .expect("run ffmpeg");
        if !generated.success() {
            eprintln!("skipping {file}: this FFmpeg can't encode it");
            continue;
        }
        let input = InputContext::open(path.to_string_lossy().to_string())
            .unwrap_or_else(|error| panic!("{file} must open: {error}"));
        let info = input.media_info();
        assert!(
            !info.video_streams.is_empty() || !info.audio_streams.is_empty(),
            "{file} has no streams"
        );
    }
}
