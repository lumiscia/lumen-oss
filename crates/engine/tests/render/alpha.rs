//! Translucent images, video and text through the engine's operators, on known pixels. Images and
//! video come from real files through the real decoders, whose pixels are straight alpha, and
//! every expected colour is worked out by hand from the premultiplied source below.

use std::{fs, process::Command};

use lumen_engine::media::{
    FfmpegResolverOptions, FfmpegVideoResolver, ImageFileResolver, ImageResolver, MediaFrame,
    VideoFrameResolver,
};
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, scene};
use crate::text_pixels;

/// The left half of a media file: transparent, with a red that must never reach the output.
const HIDDEN_RED: [u8; 4] = [255, 0, 0, 0];
/// The right half of a media file, as stored in the file.
const STRAIGHT: [u8; 4] = [60, 120, 200, 128];
/// `STRAIGHT` premultiplied: each colour channel times 128 / 255, rounded.
const TRANSLUCENT: [u8; 4] = [30, 60, 100, 128];
const CLEAR: [u8; 4] = [0; 4];

const MEDIA_SIZE: (u32, u32) = (16, 8);
const MEDIA_ROW: u32 = MEDIA_SIZE.1 / 2;
/// The first media column showing `STRAIGHT`.
const MEDIA_EDGE: u32 = MEDIA_SIZE.0 / 2;

const TEXT_SIZE: (u32, u32) = (64, 112);
/// Inside the stem of the "I", far enough from its sides for a small blur not to reach them.
const TEXT_INSIDE: (u32, u32) = (21, 56);
const TEXT_OUTSIDE: (u32, u32) = (60, 8);

const COPY_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(output, vec2<i32>(id.xy), textureLoad(input0, vec2<i32>(id.xy), 0));
}
";

/// A source raster and where to look at it.
struct Source {
    name: &'static str,
    canvas: (u32, u32),
    node: Value,
    media: TestMedia,
    /// A pixel showing the translucent colour with all of its coverage.
    inside: (u32, u32),
    /// A pixel the source doesn't cover, or covers only with transparent pixels.
    outside: (u32, u32),
    /// The levels a channel may be off by. Video passes through YUV.
    tolerance: u8,
}

/// 16 × 8 straight RGBA, half `HIDDEN_RED` and half `STRAIGHT`.
fn media_pixels() -> Vec<u8> {
    let (width, height) = MEDIA_SIZE;
    (0..width * height)
        .flat_map(|index| {
            if index % width < MEDIA_EDGE {
                HIDDEN_RED
            } else {
                STRAIGHT
            }
        })
        .collect()
}

fn image_source(directory: &std::path::Path) -> Source {
    let path = directory.join("translucent.png");
    let (width, height) = MEDIA_SIZE;
    image::RgbaImage::from_raw(width, height, media_pixels())
        .expect("pixels fit")
        .save(&path)
        .expect("write png");
    let resolver = ImageFileResolver::open(path.to_string_lossy()).expect("open png");
    let MediaFrame::CpuRgba(frame) = resolver.frame().expect("decode png") else {
        panic!("expected a CPU frame");
    };
    let media = TestMedia::default().image("plate", width, height, frame.rgba.to_vec());
    media_source("image", json!({ "source": "plate" }), media, 0)
}

/// ProRes 4444, or `None` where the FFmpeg CLI or its ProRes encoder is missing.
fn video_source(directory: &std::path::Path) -> Option<Source> {
    let raw = directory.join("translucent.rgba");
    let path = directory.join("translucent.mov");
    fs::write(&raw, media_pixels()).expect("write raw frame");
    let (width, height) = MEDIA_SIZE;
    let output = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-r", "30"])
        .args(["-s", &format!("{width}x{height}")])
        .arg("-i")
        .arg(&raw)
        .args(["-vf", "scale=out_color_matrix=bt709:out_range=tv"])
        .args(["-colorspace", "bt709", "-pix_fmt", "yuva444p10le"])
        .args([
            "-c:v",
            "prores_ks",
            "-profile:v",
            "4444",
            "-alpha_bits",
            "16",
        ])
        .args(["-qscale:v", "1"])
        .arg(&path)
        .output();
    let Ok(output) = output else {
        eprintln!("ffmpeg CLI not installed; skipping translucent video rows");
        return None;
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Unknown encoder"), "{stderr}");
        eprintln!("prores_ks unavailable; skipping translucent video rows");
        return None;
    }
    let resolver = FfmpegVideoResolver::open_with_options(
        path.to_string_lossy().to_string(),
        FfmpegResolverOptions::default(),
    )
    .expect("open video");
    let MediaFrame::Yuv(frame) = resolver.frame(0).expect("decode video") else {
        panic!("expected a YUV frame");
    };
    let media = TestMedia::default().yuv_video("plate", width, height, (*frame).clone());
    Some(media_source(
        "video",
        json!({ "kind": "video", "source": "plate" }),
        media,
        3,
    ))
}

fn media_source(name: &'static str, params: Value, media: TestMedia, tolerance: u8) -> Source {
    Source {
        name,
        canvas: MEDIA_SIZE,
        node: json!({ "id": 1, "type": "media_in", "params": params }),
        media,
        inside: (MEDIA_SIZE.0 - 2, MEDIA_ROW),
        outside: (2, MEDIA_ROW),
        tolerance,
    }
}

fn text_source() -> Source {
    Source {
        name: "text",
        canvas: TEXT_SIZE,
        node: json!({ "id": 1, "type": "text", "params": {
            "content": "I", "font_family": "Inter", "font_size": 96, "position": [10, 2],
            "color": STRAIGHT
        } }),
        media: text_pixels::media(),
        inside: TEXT_INSIDE,
        outside: TEXT_OUTSIDE,
        tolerance: 1,
    }
}

/// The source through one node of `kind`.
fn through(source: &Source, kind: &str, params: Value) -> Scene {
    scene(
        source.canvas,
        json!([source.node, { "id": 2, "type": kind, "params": params, "in": { "source": 1 } }]),
    )
}

fn at(point: (u32, u32), color: [u8; 4]) -> (u32, u32, [u8; 4]) {
    (point.0, point.1, color)
}

/// Every operator's output for one source.
fn rows(source: &Source) -> Vec<Case> {
    let (inside, outside) = (source.inside, source.outside);
    let row = |name: &str, scene: Scene, probes: Vec<(u32, u32, [u8; 4])>| {
        Case::new(
            format!("{}: {name}", source.name),
            scene,
            Expect::Probes(probes),
        )
        .media(source.media.clone())
        .tolerance(source.tolerance)
    };

    const GREY: [u8; 4] = [64, 64, 64, 255];
    let (width, height) = source.canvas;
    let base = json!({ "id": 3, "type": "background", "params": {
        "paint": GREY, "width": width, "height": height } });

    vec![
        row(
            "stored premultiplied",
            scene(source.canvas, json!([source.node])),
            vec![at(inside, TRANSLUCENT), at(outside, CLEAR)],
        ),
        // Colour + 64 × (1 - 128 / 255), which adds 31.87.
        row(
            "merge over grey",
            scene(
                source.canvas,
                json!([source.node, base, { "id": 2, "type": "merge", "params": {},
                    "in": { "base": 3, "overlay": 1 } }]),
            ),
            vec![at(inside, [62, 92, 132, 255]), at(outside, GREY)],
        ),
        // Every channel halves.
        row(
            "opacity",
            through(source, "opacity", json!({ "opacity": 0.5 })),
            vec![at(inside, [15, 30, 50, 64]), at(outside, CLEAR)],
        ),
        // Moving by whole pixels keeps the stored colour.
        row(
            "transform",
            through(
                source,
                "transform",
                json!({ "translate_y": 1, "sampling": "linear" }),
            ),
            vec![at(inside, TRANSLUCENT), at(outside, CLEAR)],
        ),
        // A shader that copies its input sees the premultiplied colour, and its output is read the
        // same way.
        row(
            "wgsl_shader input",
            through(source, "wgsl_shader", json!({ "shader": COPY_SHADER })),
            vec![at(inside, TRANSLUCENT), at(outside, CLEAR)],
        ),
        // A black shadow under the source adds alpha and no colour:
        // 128 + 128 × (1 - 128 / 255) = 192.
        row(
            "shadow",
            through(
                source,
                "shadow",
                json!({ "offset_x": 0, "offset_y": 0, "radius": 0,
                    "color": [0, 0, 0, 255], "opacity": 1 }),
            ),
            vec![at(inside, [30, 60, 100, 192]), at(outside, CLEAR)],
        ),
    ]
}

/// A half-pixel move mixes the media's edge column half and half with the transparent column beside
/// it, so the hidden red must not show.
fn filtered_edge_rows(source: &Source) -> Vec<Case> {
    let edge = (MEDIA_EDGE, MEDIA_ROW);
    vec![
        Case::new(
            format!("{}: transform filters by coverage", source.name),
            through(
                source,
                "transform",
                json!({ "translate_x": 0.5, "sampling": "linear" }),
            ),
            Expect::Probes(vec![
                at(edge, [15, 30, 50, 64]),
                at((MEDIA_EDGE + 2, MEDIA_ROW), TRANSLUCENT),
                at(source.outside, CLEAR),
            ]),
        )
        .media(source.media.clone())
        .tolerance(source.tolerance),
        // Only the shadow reaches these columns, at the source's alpha.
        Case::new(
            format!("{}: shadow of the translucent part", source.name),
            through(
                source,
                "shadow",
                json!({ "offset_x": -4, "offset_y": 0, "radius": 0,
                    "color": [0, 0, 0, 255], "opacity": 1 }),
            ),
            Expect::Probes(vec![at((MEDIA_EDGE - 2, MEDIA_ROW), [0, 0, 0, 128])]),
        )
        .media(source.media.clone())
        .tolerance(source.tolerance),
    ]
}

/// However far a blur spreads, each pixel holds the source's colour at the alpha it reached.
fn blur_row(source: &Source) -> Case {
    let (inside, outside) = (source.inside, source.outside);
    let tolerance = i32::from(source.tolerance) + 1;
    let keeps_its_colour = move |frames: &[Frame]| {
        for frame in frames {
            let mut partial = 0;
            for y in 0..frame.height {
                for x in 0..frame.width {
                    let [r, g, b, a] = frame.pixel(x, y).map(i32::from);
                    partial += i32::from(a > 16 && a < 112);
                    for (channel, full) in [(r, 30), (g, 60), (b, 100)] {
                        let expected = full * a / 128;
                        assert!(
                            (channel - expected).abs() <= tolerance,
                            "({x}, {y}) is {:?}, whose colour at alpha {a} should be {expected}, \
                             within {tolerance}",
                            frame.pixel(x, y),
                        );
                    }
                }
            }
            assert!(partial > 0, "the blur left no partly covered pixels");
            let [r, g, b, a] = frame.pixel(inside.0, inside.1);
            assert!(
                [r, g, b, a]
                    .iter()
                    .zip(TRANSLUCENT)
                    .all(|(actual, expected)| actual.abs_diff(expected) <= tolerance as u8),
                "the blur's interior is {:?}",
                [r, g, b, a]
            );
            let far = frame.pixel(outside.0, outside.1);
            assert_eq!(far[3], 0, "the blur reached {outside:?}: {far:?}");
        }
    };
    Case::new(
        format!("{}: blur", source.name),
        through(source, "blur", json!({ "radius": 1 })),
        Expect::Custom(Box::new(keeps_its_colour)),
    )
    .media(source.media.clone())
}

#[test]
fn translucent_sources_stay_premultiplied_through_every_operator() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let image = image_source(directory.path());
    let video = video_source(directory.path());
    let text = text_source();

    let mut cases = rows(&image);
    cases.extend(filtered_edge_rows(&image));
    cases.push(blur_row(&image));
    if let Some(video) = &video {
        cases.extend(rows(video));
        cases.extend(filtered_edge_rows(video));
        cases.push(blur_row(video));
    }
    cases.extend(rows(&text));
    cases.push(blur_row(&text));
    check(cases);
}
