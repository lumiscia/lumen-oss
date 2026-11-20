//! `erode_dilate` on shapes whose grown and shrunk edges are known in closed form.

use serde_json::{Value, json};

use crate::support::{Frame, TestMedia, gpu, render, scene};

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// A `size` by `size` frame of `shape` over transparent black, where `shape` gives a pixel's
/// colour or `None` outside it.
fn shape_frame(
    size: u32,
    erode_dilate: Option<Value>,
    shape: impl Fn(u32, u32) -> Option<[u8; 4]>,
) -> Frame {
    let rgba: Vec<u8> = (0..size * size)
        .flat_map(|index| shape(index % size, index / size).unwrap_or([0; 4]))
        .collect();
    let media = TestMedia::default().image("shape", size, size, rgba);
    let mut nodes = vec![
        json!({ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "shape" } }),
    ];
    if let Some(params) = erode_dilate {
        nodes.push(
            json!({ "id": 2, "type": "erode_dilate", "in": { "source": 1 }, "params": params }),
        );
    }
    let composition = scene((size, size), Value::Array(nodes)).into();
    render(&mut gpu().renderer(), &composition, 0.0, &media)
}

/// The 10 by 10 square at columns and rows 11 to 20 of a 32 pixel frame.
fn square(x: u32, y: u32) -> Option<[u8; 4]> {
    ((11..21).contains(&x) && (11..21).contains(&y)).then_some(RED)
}

#[test]
fn growing_a_square_puts_its_edges_out_and_rounds_its_corners() {
    let round = shape_frame(32, Some(json!({ "radius": 3 })), square);
    // Three pixels out on each side, and no further.
    for (x, y) in [(8, 15), (23, 15), (15, 8), (15, 23)] {
        assert_eq!(round.pixel(x, y), RED, "({x}, {y}) is within 3 pixels");
    }
    for (x, y) in [(7, 15), (24, 15), (15, 7), (15, 24)] {
        assert_eq!(round.pixel(x, y)[3], 0, "({x}, {y}) is 4 pixels out");
    }
    // The corner is 3 pixels out on each axis, 4.2 away from the square.
    assert_eq!(round.pixel(8, 8)[3], 0);
    assert_eq!(round.pixel(9, 9), RED);
    let partial = round.pixel(8, 9)[3];
    assert!(
        0 < partial && partial < 255,
        "a rounded corner is {partial}"
    );

    let square = shape_frame(32, Some(json!({ "radius": 3, "shape": "square" })), square);
    for (x, y) in [(8, 8), (23, 8), (8, 23), (23, 23)] {
        assert_eq!(square.pixel(x, y), RED, "({x}, {y}) is a square corner");
    }
    assert_eq!(square.pixel(7, 8)[3], 0);
    assert_eq!(square.pixel(8, 7)[3], 0);
}

#[test]
fn shrinking_a_square_takes_each_side_in() {
    let frame = shape_frame(32, Some(json!({ "radius": -3 })), square);
    for (x, y) in [(14, 14), (17, 14), (14, 17), (17, 17)] {
        assert_eq!(
            frame.pixel(x, y),
            RED,
            "({x}, {y}) is inside the shrunk square"
        );
    }
    for (x, y) in [(13, 15), (18, 15), (15, 13), (15, 18)] {
        assert_eq!(frame.pixel(x, y)[3], 0, "({x}, {y}) is outside it");
    }
}

#[test]
fn growing_keeps_the_colour_of_the_nearest_edge_without_dark_fringes() {
    let two_colours = |x: u32, y: u32| square(x, y).map(|_| if x < 16 { RED } else { BLUE });
    let frame = shape_frame(32, Some(json!({ "radius": 3, "softness": 2 })), two_colours);
    for x in 6..16 {
        let [r, g, b, a] = frame.pixel(x, 15);
        assert!(
            r.abs_diff(a) <= 1 && g == 0 && b == 0,
            "({x}, 15) is {r}, {g}, {b}, {a}"
        );
    }
    for x in 16..26 {
        let [r, g, b, a] = frame.pixel(x, 15);
        assert!(
            b.abs_diff(a) <= 1 && r == 0 && g == 0,
            "({x}, 15) is {r}, {g}, {b}, {a}"
        );
    }
    assert!(
        0 < frame.pixel(7, 15)[3],
        "the soft edge reaches past the radius"
    );
}

#[test]
fn a_large_radius_gives_a_disc_of_that_radius() {
    const SIZE: u32 = 512;
    const CENTRE: u32 = 256;
    const RADIUS: f64 = 200.0;
    let frame = shape_frame(SIZE, Some(json!({ "radius": RADIUS })), |x, y| {
        (x == CENTRE && y == CENTRE).then_some(RED)
    });
    for y in 0..SIZE {
        for x in 0..SIZE {
            let distance = f64::from(x.abs_diff(CENTRE)).hypot(f64::from(y.abs_diff(CENTRE)));
            let alpha = frame.pixel(x, y)[3];
            if distance <= RADIUS - 1.0 {
                assert_eq!(alpha, 255, "({x}, {y}) is {distance} from the centre");
            } else if distance >= RADIUS + 1.5 {
                assert_eq!(alpha, 0, "({x}, {y}) is {distance} from the centre");
            }
        }
    }
}

#[test]
fn no_radius_and_no_softness_pass_the_source_through() {
    let untouched = shape_frame(32, None, square);
    let passed = shape_frame(32, Some(json!({ "radius": 0, "softness": 0 })), square);
    assert_eq!(passed.rgba, untouched.rgba);
}
