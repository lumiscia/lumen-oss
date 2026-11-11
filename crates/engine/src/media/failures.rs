//! What each loader reports for a source that is missing, of another kind, unsupported or
//! corrupt: one failure shape for every kind, read through its public code.

use std::{fs, path::Path, process::Command};

use super::{AssetKind, FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver};
use crate::error::MediaError;

fn generate(dir: &Path, name: &str, args: &[&str]) -> Option<String> {
    let path = dir.join(name);
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(args)
        .arg(&path)
        .status();
    matches!(status, Ok(status) if status.success()).then(|| path.to_string_lossy().to_string())
}

/// What a source of one kind fails with when opened as `wanted`.
struct Expected {
    code: &'static str,
    found: Option<AssetKind>,
}

fn expect(error: MediaError, wanted: AssetKind, source: &str, expected: Expected, row: &str) {
    let failure = error.failure().unwrap_or_else(|| panic!("{row}: {error}"));
    assert_eq!(failure.code(), expected.code, "{row}: {error}");
    assert_eq!(failure.found, expected.found, "{row}: {error}");
    assert_eq!(failure.wanted, [wanted], "{row}: {error}");
    assert_eq!(failure.source, source, "{row}: {error}");
    assert!(error.to_string().contains(source), "{row}: {error}");
}

fn open_image(source: &str) -> MediaError {
    super::ImageFileResolver::open(source)
        .err()
        .unwrap_or_else(|| panic!("{source} opens as an image"))
}

fn open_video(source: &str) -> MediaError {
    FfmpegVideoResolver::open_with_options(source.to_string(), FfmpegResolverOptions::default())
        .err()
        .unwrap_or_else(|| panic!("{source} opens as a video"))
}

fn open_audio(source: &str) -> MediaError {
    FfmpegAudioResolver::open(source)
        .err()
        .unwrap_or_else(|| panic!("{source} opens as audio"))
}

/// Opens `source` as `wanted` and checks it fails as `expected` says.
fn check(row: &str, wanted: AssetKind, source: &str, expected: Expected) {
    let error = match wanted {
        AssetKind::Image => open_image(source),
        AssetKind::Video => open_video(source),
        _ => open_audio(source),
    };
    expect(error, wanted, source, expected, row);
}

#[test]
fn a_source_that_cant_be_used_fails_with_the_same_shape_for_every_kind() {
    use AssetKind::{Audio, Image, Video};

    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let tone = ["-i", "sine=frequency=440:duration=0.5"];
    let (Some(wav), Some(mp4), Some(m4a)) = (
        generate(dir, "tone.wav", &[tone[0], tone[1], "-c:a", "pcm_s16le"]),
        generate(
            dir,
            "clip.mp4",
            &[
                "-i",
                "color=c=red:s=16x16:d=0.5",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ],
        ),
        generate(dir, "voice.mp4", &[tone[0], tone[1], "-c:a", "aac"]),
    ) else {
        eprintln!("ffmpeg CLI unavailable; skipping the media failure table");
        return;
    };
    let webm = generate(
        dir,
        "voice.webm",
        &[tone[0], tone[1], "-c:a", "opus", "-strict", "-2"],
    );
    let path = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path.to_string_lossy().to_string()
    };
    let missing = dir.join("missing.bin").to_string_lossy().to_string();
    let garbage = path("garbage.bin", b"these bytes are no media at all, only text");
    let playlist = path(
        "list.m3u8",
        b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-ENDLIST\n",
    );
    let heic = path("photo.heic", b"\0\0\0\x18ftypheic\0\0\0\0mif1heic");
    let truncated = {
        let mut png = Vec::new();
        ::image::DynamicImage::new_rgba8(8, 8)
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                ::image::ImageFormat::Png,
            )
            .unwrap();
        png.truncate(png.len() / 2);
        path("cut.png", &png)
    };
    let fails = |code, found| Expected { code, found };
    let not_found = || fails("media.not_found", None);
    let decode = || fails("media.decode", None);
    let unsupported = || fails("media.unsupported", None);
    // A container's header can't tell video from audio, so only opening it says what it holds.
    let wrong = |found| fails("media.wrong_kind", found);
    let (audio, video) = (Some(Audio), Some(Video));

    check("image: missing", Image, &missing, not_found());
    check("image: a video", Image, &mp4, wrong(None));
    check("image: an audio-only mp4", Image, &m4a, wrong(None));
    check("image: audio", Image, &wav, wrong(audio));
    check("image: HEIC", Image, &heic, unsupported());
    check("image: not an image", Image, &garbage, decode());
    check("image: truncated", Image, &truncated, decode());
    check("video: missing", Video, &missing, not_found());
    check("video: audio", Video, &wav, wrong(audio));
    check("video: an audio-only mp4", Video, &m4a, wrong(audio));
    check("video: playlist", Video, &playlist, unsupported());
    check("video: not media", Video, &garbage, decode());
    check("audio: missing", Audio, &missing, not_found());
    check("audio: a silent video", Audio, &mp4, wrong(video));
    check("audio: playlist", Audio, &playlist, unsupported());
    check("audio: not media", Audio, &garbage, decode());
    if let Some(webm) = webm {
        check("image: an audio-only webm", Image, &webm, wrong(None));
        check("video: an audio-only webm", Video, &webm, wrong(audio));
    }
}
