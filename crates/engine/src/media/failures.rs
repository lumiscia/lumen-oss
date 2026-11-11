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

#[test]
fn a_source_that_cant_be_used_fails_with_the_same_shape_for_every_kind() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let (Some(wav), Some(mp4)) = (
        generate(
            dir,
            "tone.wav",
            &["-i", "sine=frequency=440:duration=0.5", "-c:a", "pcm_s16le"],
        ),
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
    ) else {
        eprintln!("ffmpeg CLI unavailable; skipping the media failure table");
        return;
    };
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
    let not_found = || Expected {
        code: "media.not_found",
        found: None,
    };
    let decode = || Expected {
        code: "media.decode",
        found: None,
    };
    let unsupported = || Expected {
        code: "media.unsupported",
        found: None,
    };
    let wrong = |found| Expected {
        code: "media.wrong_kind",
        found: Some(found),
    };

    let rows = [
        (
            "image: missing",
            AssetKind::Image,
            &missing,
            open_image(&missing),
            not_found(),
        ),
        (
            "image: a video",
            AssetKind::Image,
            &mp4,
            open_image(&mp4),
            wrong(AssetKind::Video),
        ),
        (
            "image: audio",
            AssetKind::Image,
            &wav,
            open_image(&wav),
            wrong(AssetKind::Audio),
        ),
        (
            "image: HEIC",
            AssetKind::Image,
            &heic,
            open_image(&heic),
            unsupported(),
        ),
        (
            "image: not an image",
            AssetKind::Image,
            &garbage,
            open_image(&garbage),
            decode(),
        ),
        (
            "image: truncated",
            AssetKind::Image,
            &truncated,
            open_image(&truncated),
            decode(),
        ),
        (
            "video: missing",
            AssetKind::Video,
            &missing,
            open_video(&missing),
            not_found(),
        ),
        (
            "video: audio",
            AssetKind::Video,
            &wav,
            open_video(&wav),
            wrong(AssetKind::Audio),
        ),
        (
            "video: playlist",
            AssetKind::Video,
            &playlist,
            open_video(&playlist),
            unsupported(),
        ),
        (
            "video: not media",
            AssetKind::Video,
            &garbage,
            open_video(&garbage),
            decode(),
        ),
        (
            "audio: missing",
            AssetKind::Audio,
            &missing,
            open_audio(&missing),
            not_found(),
        ),
        (
            "audio: a silent video",
            AssetKind::Audio,
            &mp4,
            open_audio(&mp4),
            wrong(AssetKind::Video),
        ),
        (
            "audio: playlist",
            AssetKind::Audio,
            &playlist,
            open_audio(&playlist),
            unsupported(),
        ),
        (
            "audio: not media",
            AssetKind::Audio,
            &garbage,
            open_audio(&garbage),
            decode(),
        ),
    ];
    for (row, wanted, source, error, expected) in rows {
        expect(error, wanted, source, expected, row);
    }
}
