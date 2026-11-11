//! The `motion_blur` node, which renders its source at sub-frames across its shutter and averages
//! them.

use lumen_engine::{
    composition::Composition, gpu::GpuCompositionRenderer, media::collect_frame_requirements,
};
use serde_json::{Value, json};

use crate::color::{linear_average, srgb8};
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

/// A shape or path that stays put while the copies it repeats spread apart, with an analytic
/// shutter of `motion_blur`.
fn spreading(kind: &str, motion_blur: f64) -> Value {
    let mut params = match kind {
        "shape" => json!({ "width": 8, "height": 8 }),
        _ => json!({ "data": "M 0 0 L 8 0 L 4 8 Z" }),
    };
    params.as_object_mut().unwrap().extend(
        json!({
            "position": [4, 28],
            "fill_paint": ORANGE,
            "motion_blur": motion_blur,
            "repeat": { "count": 4, "position_offset": "=vec2(4 + frame * 3, 0)" },
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    json!({ "id": 1, "type": kind, "params": params })
}

/// Whether `blurred` is `average` to within a code. The average is of sub-frames rounded to bytes,
/// and the blur averages its samples before rounding.
fn is_average(blurred: &[u8], average: &[u8]) -> bool {
    blurred.len() == average.len()
        && blurred
            .iter()
            .zip(average)
            .all(|(&blurred, &average)| blurred.abs_diff(average) <= 1)
}

/// The linear-light mean of `source` rendered at the 8 sub-frames of a 180° shutter around `at`.
fn sub_frame_average(
    renderer: &mut GpuCompositionRenderer,
    source: &Composition,
    at: f64,
) -> Vec<u8> {
    let media = TestMedia::default();
    let sub_frames: Vec<Frame> = (0..8)
        .map(|sample| at + ((2 * sample + 1) as f64 / 8.0 - 1.0) * 0.25)
        .map(|sub_frame| render(renderer, source, sub_frame, &media))
        .collect();
    let frames: Vec<&[u8]> = sub_frames.iter().map(|frame| frame.rgba.as_slice()).collect();
    linear_average(&frames)
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
    assert!(
        is_average(&blurred.rgba, &sub_frame_average(&mut renderer, &source, at)),
        "the blur is not the average of its sub-frames"
    );
    let held = render(&mut renderer, &source, at, &media);
    assert!(blurred.rgba != held.rgba, "the blur matches the held shape");
}

#[test]
fn repeated_copies_blur_once_across_the_sub_frames() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let at = 3.0;
    for kind in ["shape", "path"] {
        // The copies' own shutter would blur each sample again on top of the average.
        let blurred = render(
            &mut renderer,
            &blurred(8, 180.0, spreading(kind, 180.0)).into(),
            at,
            &media,
        );
        let source: Composition = canvas(json!([spreading(kind, 0.0)])).into();
        assert!(
            is_average(&blurred.rgba, &sub_frame_average(&mut renderer, &source, at)),
            "the {kind}'s blurred copies are not the average of their sub-frames"
        );
        let held = render(&mut renderer, &source, at, &media);
        assert!(
            blurred.rgba != held.rgba,
            "the {kind}'s blurred copies match the held copies"
        );
    }
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
    // Half the samples fall before frame 1, showing video `a`, and half show frame 0 of `b`: half
    // the light of each.
    let switching = scene(
        (1, 1),
        json!([
            { "id": 1, "type": "media_in", "params": {
                "kind": "video", "source": "=if(frame < 1, 'a', 'b')", "speed": 0 } },
            motion_blur(2, 1, 8, 180.0),
        ]),
    )
    .frames(4);
    check(vec![
        Case::new(
            "video at a frame whose sub-frames show several source frames",
            video(),
            Expect::Uniform([1, 0, 0, 255]),
        )
        .media(media)
        .tolerance(0),
        Case::new(
            "video whose source changes across the shutter",
            switching,
            Expect::Uniform([srgb8(0.5), 0, srgb8(0.5), 255]),
        )
        .media(
            TestMedia::default()
                .still_video("a", 1, 1, vec![255, 0, 0, 255])
                .still_video("b", 1, 1, vec![0, 0, 255, 255]),
        )
        .frames([1.0])
        .tolerance(0),
    ]);
}

#[test]
fn blank_samples_leave_the_domain_where_the_source_draws() {
    // The shape shows from frame 1, so the first half of the samples at frame 1 are blank, and all
    // of them at frame 0.
    let shape = json!({ "id": 1, "type": "shape", "params": {
        "width": 8, "height": 8, "position": [224, 224], "fill_paint": ORANGE,
    } });
    let appearing = json!({ "id": 2, "type": "switch",
        "params": { "selected_layer": "=if(frame < 1, -1, 0)" }, "in": { "layers": [1] } });
    let blurred: Composition = scene(
        (256, 256),
        json!([shape.clone(), appearing, motion_blur(3, 2, 8, 180.0)]),
    )
    .frames(4)
    .into();
    let still: Composition = scene((256, 256), json!([shape])).frames(4).into();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let frame = render(&mut renderer, &blurred, 1.0, &media);
    let domain = renderer
        .recorded_domain(&blurred, lumen_engine::node::NodeId::new(3))
        .expect("the blur recorded");
    let held = render(&mut renderer, &still, 1.0, &media);
    let drawn = renderer
        .recorded_domain(&still, lumen_engine::node::NodeId::new(1))
        .expect("the shape recorded");
    assert_eq!(domain, drawn, "the blur's domain is not the shape's");
    let blank = vec![0; held.rgba.len()];
    let half = linear_average(&[&held.rgba, &held.rgba, &blank, &blank]);
    assert!(is_average(&frame.rgba, &half), "the blur is not half the shape");
    let hidden = render(&mut renderer, &blurred, 0.0, &media);
    assert!(
        hidden.rgba.iter().all(|&channel| channel == 0),
        "samples that are all blank draw something"
    );
}
