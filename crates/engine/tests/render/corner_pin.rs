//! `corner_pin`: the pin of a raster onto four points, its edges, its filtering and its motion
//! blur.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, TestMedia, gpu, render, scene};

/// The code of the grey that is half the light of white once encoded.
const MID_GREY: u8 = 188;

fn plate_node() -> Value {
    json!({ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } })
}

/// `plate` through a node of type `kind` with `params`, on a square canvas of `canvas` pixels.
fn through(canvas: u32, kind: &str, params: Value) -> Composition {
    scene(
        (canvas, canvas),
        json!([
            plate_node(),
            { "id": 2, "type": kind, "in": { "source": 1 }, "params": params },
        ]),
    )
    .into()
}

fn pin(canvas: u32, corners: [[f64; 2]; 4]) -> Composition {
    through(canvas, "corner_pin", corners_of(corners))
}

fn corners_of([top_left, top_right, bottom_right, bottom_left]: [[f64; 2]; 4]) -> Value {
    json!({
        "top_left": top_left, "top_right": top_right,
        "bottom_right": bottom_right, "bottom_left": bottom_left,
    })
}

/// An opaque grey image of `size` pixels square, the grey of each pixel given by `grey(x, y)`.
fn grey_plate(size: u32, grey: impl Fn(u32, u32) -> u8) -> TestMedia {
    let pixels = (0..size * size)
        .flat_map(|index| {
            let value = grey(index % size, index / size);
            [value, value, value, 255]
        })
        .collect();
    TestMedia::default().image("plate", size, size, pixels)
}

fn draw(composition: &Composition, media: &TestMedia) -> Frame {
    render(&mut gpu().renderer(), composition, 0.0, media)
}

fn render_error(composition: &Composition) -> String {
    gpu()
        .renderer()
        .render(
            composition,
            0.0,
            &grey_plate(8, |_, _| 255),
            RenderTarget::Owned,
        )
        .expect_err("the render fails")
        .to_string()
}

#[test]
fn corners_at_the_sources_own_rect_reproduce_it() {
    let media = TestMedia::default().image(
        "plate",
        64,
        64,
        (0..64 * 64)
            .flat_map(|index| [(index % 64 * 4) as u8, (index / 64 * 4) as u8, 128, 255])
            .collect(),
    );
    let own = pin(64, [[0.0, 0.0], [64.0, 0.0], [64.0, 64.0], [0.0, 64.0]]);
    let plain: Composition = scene((64, 64), json!([plate_node()])).into();
    crate::support::check(vec![
        Case::new("own corners", own, Expect::Matches(plain))
            .media(media)
            .tolerance(2),
    ]);
}

/// The pixels of `frame` at least `margin` inside the `[left, top, right, bottom]` box.
fn inside(frame: &Frame, [left, top, right, bottom]: [u32; 4], margin: u32) -> Vec<(u32, u32)> {
    (top + margin..bottom - margin)
        .flat_map(|y| (left + margin..right - margin).map(move |x| (x, y)))
        .filter(|&(x, y)| x < frame.width && y < frame.height)
        .collect()
}

/// The grey of the texel at `(x, y)`.
type Texel = fn(u32, u32) -> u8;

#[test]
fn a_scaled_and_translated_rectangle_matches_transform() {
    let size = 256;
    let (translate_x, translate_y) = (20.0, 10.0);
    // Stripes 20 texels wide, shown at half size; then a smooth gradient, which `transform` and
    // the pin agree on whatever level of the source each reads, shown at a tenth of it.
    let sources: [(f64, Texel); 2] = [
        (0.5, |x, y| {
            if (x / 20) % 2 == 0 {
                255 - (y / 2) as u8
            } else {
                40
            }
        }),
        (0.1, |x, y| ((x + y) / 2) as u8),
    ];
    for (scale, texel) in sources {
        // `transform` scales about the source's centre, then translates.
        let centre = f64::from(size) / 2.0;
        let corner = |x: f64, y: f64| {
            [
                (x - centre) * scale + centre + translate_x,
                (y - centre) * scale + centre + translate_y,
            ]
        };
        let far = f64::from(size);
        let rect = [
            corner(0.0, 0.0),
            corner(far, 0.0),
            corner(far, far),
            corner(0.0, far),
        ];
        let pinned = draw(&pin(size, rect), &grey_plate(size, texel));
        let moved = draw(
            &through(
                size,
                "transform",
                json!({
                    "scale_x": scale, "scale_y": scale,
                    "translate_x": translate_x, "translate_y": translate_y,
                }),
            ),
            &grey_plate(size, texel),
        );
        let [left, top, right, bottom] =
            [rect[0][0], rect[0][1], rect[2][0], rect[2][1]].map(|value| value.round() as u32);
        let pixels = inside(&pinned, [left, top, right, bottom], 2);
        assert!(
            pixels.len() > 100,
            "scale {scale}: the pin is too small to compare"
        );
        for (x, y) in pixels {
            let (a, b) = (pinned.pixel(x, y), moved.pixel(x, y));
            assert!(
                a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 4),
                "scale {scale}: ({x}, {y}) is {a:?} pinned and {b:?} transformed"
            );
        }
    }
}

/// The lengths of the runs of light and dark along row `y` of `frame` between `from` and `to`.
fn runs(frame: &Frame, y: u32, from: u32, to: u32) -> Vec<u32> {
    let mut runs = Vec::new();
    let mut light = None;
    for x in from..to {
        let now = frame.pixel(x, y)[0] > 127;
        if light == Some(now) {
            *runs.last_mut().unwrap() += 1;
        } else {
            runs.push(1);
            light = Some(now);
        }
    }
    runs
}

#[test]
fn a_trapezoid_pin_shrinks_the_far_end_and_antialiases_its_edges() {
    // 8 by 8 cells of 8 texels, pinned narrow at the top and wide at the bottom.
    let cells = grey_plate(64, |x, y| if (x / 8 + y / 8) % 2 == 0 { 255 } else { 0 });
    let frame = draw(
        &pin(
            128,
            [[44.0, 20.0], [84.0, 20.0], [124.0, 100.0], [4.0, 100.0]],
        ),
        &cells,
    );
    // The first cell of a row is about an eighth of the quad's width there. The leading edge of
    // the row is antialiased, so it is left out of the run.
    let first_cell = |y: u32, left: u32| runs(&frame, y, left, left + 30)[0];
    let near = first_cell(24, 44);
    let far = first_cell(96, 8);
    assert!(
        (3..=8).contains(&near),
        "cell at the narrow end is {near} pixels wide"
    );
    assert!(
        far >= 2 * near,
        "cell at the wide end is {far} pixels wide, the narrow end's {near}"
    );

    assert_eq!(frame.pixel(2, 2)[3], 0, "outside the quad is transparent");
    assert_eq!(
        frame.pixel(120, 30)[3],
        0,
        "outside the quad is transparent"
    );
    assert_eq!(frame.pixel(64, 60)[3], 255, "inside the quad is opaque");
    // The left side runs from (44, 20) to (4, 100): about x = 24 at y = 60.
    let alphas: Vec<u8> = (14..34).map(|x| frame.pixel(x, 60)[3]).collect();
    assert!(
        alphas.iter().any(|alpha| (16..240).contains(alpha)),
        "no pixel on the edge is partly covered: {alphas:?}"
    );

    // Single-texel squares shrunk to a sliver at the far end average to grey instead of aliasing.
    let fine = grey_plate(128, |x, y| if (x + y) % 2 == 0 { 255 } else { 0 });
    let frame = draw(
        &pin(
            128,
            [[60.0, 20.0], [68.0, 20.0], [120.0, 100.0], [8.0, 100.0]],
        ),
        &fine,
    );
    for (x, y) in [(64, 26), (62, 28), (66, 30)] {
        let pixel = frame.pixel(x, y);
        assert!(
            pixel[..3]
                .iter()
                .all(|channel| channel.abs_diff(MID_GREY) <= 14)
                && pixel[3] == 255,
            "({x}, {y}) is {pixel:?}, not the average grey {MID_GREY}"
        );
    }
}

#[test]
fn corners_that_are_not_a_convex_quad_are_errors() {
    // Top right and bottom right swapped, so the quad crosses itself.
    let crossed = render_error(&pin(
        64,
        [[0.0, 0.0], [60.0, 60.0], [60.0, 0.0], [0.0, 60.0]],
    ));
    assert!(
        crossed.contains("corner_pin") && crossed.contains("convex") && crossed.contains("cross"),
        "{crossed}"
    );
    // The bottom right pushed in past the diagonal.
    let bent = render_error(&pin(
        64,
        [[0.0, 0.0], [60.0, 0.0], [20.0, 20.0], [0.0, 60.0]],
    ));
    assert!(
        bent.contains("convex") && bent.contains("bottom_right"),
        "{bent}"
    );
    // Three corners in a line.
    let flat = render_error(&pin(
        64,
        [[0.0, 0.0], [30.0, 0.0], [60.0, 0.0], [0.0, 60.0]],
    ));
    assert!(flat.contains("top_right"), "{flat}");
}

#[test]
fn animated_corners_smear_along_their_motion_with_motion_blur() {
    // A white square that moves 12 pixels right each frame.
    let moving = |motion_blur: f64| -> Composition {
        let params = json!({
            "top_left": "=vec2(10 + frame * 12, 20)",
            "top_right": "=vec2(40 + frame * 12, 20)",
            "bottom_right": "=vec2(40 + frame * 12, 50)",
            "bottom_left": "=vec2(10 + frame * 12, 50)",
            "motion_blur": motion_blur,
        });
        scene(
            (96, 64),
            json!([
                plate_node(),
                { "id": 2, "type": "corner_pin", "in": { "source": 1 }, "params": params },
            ]),
        )
        .frames(4)
        .into()
    };
    let media = grey_plate(8, |_, _| 255);
    let mut renderer = gpu().renderer();
    let sharp = render(&mut renderer, &moving(0.0), 1.0, &media);
    let blurred = render(&mut renderer, &moving(180.0), 1.0, &media);
    let partly = |frame: &Frame, pixels: &mut dyn Iterator<Item = (u32, u32)>| {
        pixels
            .filter(|&(x, y)| (16..240).contains(&frame.pixel(x, y)[3]))
            .count()
    };
    // Along the motion, across the square's leading edge at x = 52, which blurs over a half
    // frame's 6 pixels either side of it. Across it, along the top edge, nothing changes.
    let row = || (30..80).map(|x| (x, 35));
    let column = || (10..30).map(|y| (30, y));
    assert!(partly(&sharp, &mut row()) <= 2, "sharp edge");
    assert!(partly(&blurred, &mut row()) >= 4, "blurred edge");
    assert!(
        partly(&blurred, &mut column()) <= 2,
        "edge across the motion"
    );
    assert_eq!(
        blurred.pixel(40, 35)[3],
        255,
        "the square's middle stays solid"
    );
}
