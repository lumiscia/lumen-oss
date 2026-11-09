//! The `motion_blur` node, which renders its source at sub-frames across its shutter and averages
//! them.

use lumen_engine::{composition::Composition, media::collect_frame_requirements};
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

const ORANGE: [u8; 4] = [255, 128, 0, 255];

/// A shape whose width and position both animate, with an analytic shutter of `motion_blur`.
fn growing(motion_blur: f64) -> Value {
    json!({ "id": 1, "type": "shape", "params": {
        "width": "=8 + frame * 6",
        "height": 20,
        "position": "=vec2(10 + frame * 4, 32)",
        "fill_paint": ORANGE,
        "motion_blur": motion_blur,
    } })
}

fn canvas(nodes: Value) -> Scene {
    scene((64, 64), nodes).frames(8)
}

fn motion_blur(id: u32, source: u32, samples: i64, shutter: f64) -> Value {
    json!({ "id": id, "type": "motion_blur", "params": { "samples": samples, "shutter": shutter },
        "in": { "source": source } })
}

/// `source`, with id 1, under a `motion_blur`.
fn blurred(samples: i64, shutter: f64, source: Value) -> Scene {
    canvas(json!([source, motion_blur(2, 1, samples, shutter)]))
}

#[test]
fn an_animated_width_blurs_to_the_average_of_its_sub_frames() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let at = 3.0;
    let blurred = render(
        &mut renderer,
        &blurred(8, 180.0, growing(0.0)).into(),
        at,
        &media,
    );
    let source: Composition = canvas(json!([growing(0.0)])).into();
    let sub_frames: Vec<Frame> = (0..8)
        .map(|sample| at + ((2 * sample + 1) as f64 / 8.0 - 1.0) * 0.25)
        .map(|sub_frame| render(&mut renderer, &source, sub_frame, &media))
        .collect();
    let average: Vec<u8> = (0..blurred.rgba.len())
        .map(|index| {
            let sum: u32 = sub_frames.iter().map(|f| u32::from(f.rgba[index])).sum();
            (f64::from(sum) / 8.0).round() as u8
        })
        .collect();
    assert!(
        blurred.rgba == average,
        "the blur is not the average of its sub-frames"
    );
    let held = render(&mut renderer, &source, at, &media);
    assert!(blurred.rgba != held.rgba, "the blur matches the held shape");
}

#[test]
fn renders_once_unless_it_samples() {
    let unwrapped = || canvas(json!([growing(180.0)])).into();
    let same = |name: &str, scene: Scene, like: Composition| {
        Case::new(name, scene, Expect::Matches(like))
            .frames([3.0])
            .tolerance(0)
    };
    check(vec![
        same(
            "one sample is the source at the frame",
            blurred(1, 180.0, growing(180.0)),
            unwrapped(),
        ),
        same(
            "a closed shutter is the source at the frame",
            blurred(8, 0.0, growing(180.0)),
            unwrapped(),
        ),
        same(
            "analytic motion blur is off while it samples",
            blurred(8, 180.0, growing(180.0)),
            blurred(8, 180.0, growing(0.0)).into(),
        ),
        same(
            "a motion_blur inside one that samples renders once per sample",
            canvas(json!([
                growing(0.0),
                motion_blur(2, 1, 4, 360.0),
                motion_blur(3, 2, 8, 180.0),
            ])),
            blurred(8, 180.0, growing(0.0)).into(),
        ),
    ]);
}

#[test]
fn video_blurs_across_the_source_frames_its_sub_frames_show() {
    // Source frame n is red n, played 8 source frames a composition frame. At frame 0 the samples
    // fall at -3/8, -1/8, 1/8 and 3/8 of a frame, which show source frames 0, 0, 1 and 3.
    let video = || {
        scene(
            (1, 1),
            json!([
                { "id": 1, "type": "media_in",
                  "params": { "kind": "video", "source": "v", "speed": 8 } },
                motion_blur(2, 1, 4, 360.0),
            ]),
        )
        .frames(4)
    };
    let media = TestMedia::default().numbered_video("v");
    let required = collect_frame_requirements(&video().into(), &media, 0).unwrap();
    assert_eq!(required.videos[0].frames, [0, 1, 3]);
    check(vec![
        Case::new(
            "video at a frame whose sub-frames show several source frames",
            video(),
            Expect::Uniform([1, 0, 0, 255]),
        )
        .media(media)
        .tolerance(0),
    ]);
}
