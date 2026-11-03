//! One small row of each table form, and the guarantees the harness itself makes.

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::json;

use crate::support::{
    Case, Expect, Scene, TestMedia, check, check_matching, gpu, panic_message, scene,
    within_rounding,
};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn solid((width, height): (u32, u32), color: [u8; 4]) -> Scene {
    scene(
        (width, height),
        json!([{ "id": 1, "type": "background", "params": {
            "paint": color, "width": width, "height": height
        } }]),
    )
}

/// A 14 × 11 translucent orange shape at `position`, through `effect`, on a 384 × 288 canvas.
fn shape_through(position: serde_json::Value, effect: serde_json::Value) -> Scene {
    let mut nodes = vec![json!({ "id": 1, "type": "shape", "params": {
        "position": position, "width": 14, "height": 11, "fill_paint": [255, 100, 0, 200]
    } })];
    let mut effect = effect;
    effect["id"] = json!(2);
    effect["in"] = json!({ "source": 1 });
    nodes.push(effect);
    scene((384, 288), json!(nodes))
}

#[test]
fn colors_are_checked_at_every_pixel_or_at_probes() {
    let layers = |order: [u32; 2]| {
        scene(
            (4, 4),
            json!([
                { "id": 1, "type": "background", "params": { "paint": RED, "width": 4, "height": 4 } },
                { "id": 2, "type": "background", "params": { "paint": GREEN, "width": 4, "height": 4 } },
                { "id": 3, "type": "raster_multimerge", "in": { "layers": order } },
            ]),
        )
    };
    check(vec![
        Case::new(
            "opacity scales a premultiplied solid",
            scene(
                (4, 4),
                json!([
                    { "id": 1, "type": "background", "params": {
                        "paint": [200, 100, 50, 255], "width": 4, "height": 4
                    } },
                    { "id": 2, "type": "opacity", "params": { "opacity": 0.5 }, "in": { "source": 1 } },
                ]),
            ),
            Expect::Uniform([100, 50, 25, 128]),
        ),
        Case::new(
            "variadic inputs keep their order, last on top",
            layers([1, 2]),
            Expect::Uniform(GREEN),
        ),
        Case::new(
            "variadic inputs keep their order, reversed",
            layers([2, 1]),
            Expect::Uniform(RED),
        ),
        Case::new(
            "shape covers only its own pixels",
            scene(
                (4, 4),
                json!([{ "id": 1, "type": "shape", "params": {
                    "position": [1, 1], "width": 2, "height": 2,
                    "fill_paint": RED, "edge_antialias": false
                } }]),
            ),
            Expect::Probes(vec![(1, 1, RED), (2, 2, RED), (0, 0, CLEAR), (3, 3, CLEAR)]),
        ),
    ]);
}

#[test]
fn frames_share_a_renderer_and_may_each_have_an_expectation() {
    check(vec![Case::new(
        "animated crop width",
        scene(
            (8, 4),
            json!([
                { "id": 1, "type": "background", "params": { "paint": RED, "width": 8, "height": 4 } },
                { "id": 2, "type": "crop", "in": { "source": 1 }, "params": {
                    "x": 2, "y": 1, "width": "=frame", "height": 2
                } },
            ]),
        )
        .frames(6),
        Expect::PerFrame(vec![
            Expect::AlphaRows(vec!["........", "..##....", "..##....", "........"]),
            Expect::AlphaRows(vec!["........", "..#####.", "..#####.", "........"]),
        ]),
    )
    .frames([2.0, 5.0])]);
}

#[test]
fn scenes_can_be_compared_or_inspected() {
    let shape = |x: u32, y: u32| {
        json!({ "id": 1, "type": "shape", "params": {
            "position": [x, y], "width": 3, "height": 2, "fill_paint": GREEN
        } })
    };
    check(vec![
        Case::new(
            "translating a shape matches drawing it there",
            scene(
                (16, 16),
                json!([
                    shape(1, 1),
                    { "id": 2, "type": "transform", "in": { "source": 1 }, "params": {
                        "translate_x": 6, "translate_y": 4
                    } },
                ]),
            ),
            Expect::Matches(scene((16, 16), json!([shape(7, 5)])).into()),
        ),
        Case::new(
            "blur falls off with distance",
            shape_through(
                json!([100, 100]),
                json!({ "type": "blur", "params": { "radius": 6 } }),
            ),
            Expect::Custom(Box::new(|frames| {
                let alpha = |x, y| frames[0].pixel(x, y)[3];
                let (center, edge, far) = (alpha(107, 105), alpha(100, 105), alpha(60, 105));
                assert!(
                    center > edge && edge > far && far == 0,
                    "alpha at center {center}, edge {edge}, far {far}"
                );
            })),
        ),
    ]);
}

#[test]
fn media_comes_from_one_store() {
    check(vec![
        Case::new(
            "image",
            scene(
                (2, 1),
                json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "a" } }]),
            ),
            Expect::Probes(vec![(0, 0, RED), (1, 0, GREEN)]),
        )
        .media(TestMedia::default().image("a", 2, 1, [RED, GREEN].concat()))
        .tolerance(0),
        Case::new(
            "video frame follows the timeline's frame rate",
            scene(
                (1, 1),
                json!([{ "id": 1, "type": "media_in", "params": { "kind": "video", "source": "v" } }]),
            )
            .frames(11)
            .fps(60.0),
            Expect::PerFrame(vec![
                Expect::Uniform([0, 0, 0, 255]),
                Expect::Uniform([5, 0, 0, 255]),
            ]),
        )
        .media(TestMedia::default().numbered_video("v"))
        .frames([0.0, 10.0])
        .tolerance(0),
    ]);
}

#[test]
fn bounds_match_reference() {
    let blur = json!({ "type": "blur", "params": { "radius": 8.5 } });
    let shadow = json!({ "type": "shadow", "params": {
        "offset_x": 3.5, "offset_y": -2.25, "radius": 5, "color": [20, 40, 200, 255], "opacity": 0.8
    } });
    check(vec![
        Case::new(
            "blur in a corner",
            shape_through(json!([2, 3]), blur.clone()),
            Expect::Nonblank,
        )
        .bounded(true)
        .own_programs(),
        Case::new(
            "shadow with fractional offsets",
            shape_through(json!([190, 140]), shadow),
            Expect::Nonblank,
        )
        .bounded(true)
        .own_programs(),
        Case::new(
            "blur of a source moving along the timeline",
            shape_through(json!("=vec2(20 + frame * 10, 30)"), blur.clone()).frames(4),
            Expect::Nonblank,
        )
        .frames([0.0, 3.0])
        .bounded(true)
        .own_programs(),
        Case::new(
            "blur bound off the timeline's whole frames fills the canvas",
            shape_through(json!("=vec2(20 + frame * 10, 30)"), blur).frames(4),
            Expect::Nonblank,
        )
        .frames([1.5])
        .bounded(false)
        .own_programs(),
    ]);
}

#[test]
fn failures_name_every_failed_row_and_the_filter_selects_rows() {
    let rows = || {
        vec![
            Case::new("wrong color", solid((2, 2), RED), Expect::Uniform(GREEN)),
            Case::new("right color", solid((2, 2), RED), Expect::Uniform(RED)),
            Case::new(
                "wrong bound",
                shape_through(
                    json!([2, 3]),
                    json!({ "type": "blur", "params": { "radius": 3 } }),
                ),
                Expect::Nonblank,
            )
            .bounded(false),
            Case::new("blank", solid((2, 2), CLEAR), Expect::Nonblank),
        ]
    };
    let cases = rows();
    let failure = catch_unwind(AssertUnwindSafe(|| check_matching(cases, None))).unwrap_err();
    let message = panic_message(&*failure);
    assert!(message.starts_with("3 of 4 rows failed"), "{message}");
    assert!(
        message.contains("wrong color: frame 0: pixel (0, 0)"),
        "{message}"
    );
    assert!(message.contains("wrong bound: "), "{message}");
    assert!(message.contains("blank: frame 0 is blank"), "{message}");
    assert!(!message.contains("right color"), "{message}");

    check_matching(rows(), Some("right"));
}

#[test]
fn a_test_that_panics_holding_the_device_leaves_it_usable() {
    std::thread::spawn(|| {
        let _gpu = gpu();
        panic!("a failing test");
    })
    .join()
    .unwrap_err();

    check_matching(
        vec![Case::new("after", solid((2, 2), RED), Expect::Uniform(RED))],
        None,
    );
}

#[test]
fn rounding_oracle_allows_only_rare_single_steps() {
    let reference = vec![100; 4000];
    let with = |changes: &[(usize, u8)]| {
        let mut pixels = reference.clone();
        for &(index, value) in changes {
            pixels[index] = value;
        }
        pixels
    };
    let off_by_one = |count: usize| (0..count).map(|index| (index, 101)).collect::<Vec<_>>();

    assert_eq!(within_rounding(&reference, &reference), Ok(()));
    assert_eq!(within_rounding(&with(&off_by_one(4)), &reference), Ok(()));
    assert_eq!(
        within_rounding(&with(&off_by_one(5)), &reference),
        Err("5 of 4000 bytes differ".to_string())
    );
    assert_eq!(
        within_rounding(&with(&[(7, 102)]), &reference),
        Err("byte 7 (pixel 1, channel 3) is 102, expected 100".to_string())
    );
    assert!(within_rounding(&reference[..4], &reference).is_err());
    assert!(within_rounding(&[], &[]).is_err());
}

#[test]
#[should_panic(expected = "a case needs at least one frame")]
fn a_case_with_no_frames_fails() {
    check(vec![
        Case::new("no frames", solid((8, 8), RED), Expect::Nonblank).frames([]),
    ]);
}
