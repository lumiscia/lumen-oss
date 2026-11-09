//! Displacement maps on known pixels: a map byte of `128 + n` with an amount of 128 moves the
//! source `n` pixels, so every expected colour is worked out by hand.

use serde_json::{Value, json};

use crate::support::{Case, Expect, Scene, check, scene};

const WIDTH: u32 = 32;
const HEIGHT: u32 = 8;
const WHITE: [u8; 4] = [255; 4];
const CLEAR: [u8; 4] = [0; 4];

/// A white bar over columns 8 to 11.
fn bar() -> Value {
    json!({ "id": 1, "type": "shape", "params": {
        "position": [8, 0], "width": 4, "height": HEIGHT, "fill_paint": WHITE } })
}

fn solid_map(color: [u8; 4], width: u32, height: u32) -> Value {
    json!({ "id": 2, "type": "background", "params": {
        "paint": color, "width": width, "height": height } })
}

fn displaced(source: Value, map: Value, params: Value) -> Scene {
    scene(
        (WIDTH, HEIGHT),
        json!([source, map, { "id": 3, "type": "displacement_map", "params": params,
            "in": { "source": 1, "map": 2 } }]),
    )
}

/// The middle row from column 4, where each byte is the alpha of white.
fn row_from_4(alphas: [u8; 16]) -> Expect {
    let probes = (4..).zip(alphas).map(|(x, a)| (x, HEIGHT / 2, [a; 4]));
    Expect::Probes(probes.collect())
}

/// The bar moved `columns` pixels right.
fn moved(columns: i32) -> Expect {
    let mut alphas = [0; 16];
    for column in 8..12 {
        alphas[(column + columns - 4) as usize] = 255;
    }
    row_from_4(alphas)
}

#[test]
fn displacement_moves_by_the_map() {
    let red = |red: u8| solid_map([red, 0, 0, 255], 0, 0);
    let across = |amount: i32| json!({ "amount_x": amount, "amount_y": 0 });
    check(vec![
        Case::new(
            "red 132 moves 4 right, nearest",
            displaced(
                bar(),
                red(132),
                json!({ "amount_x": 128, "amount_y": 0, "sampling": "nearest" }),
            ),
            moved(4),
        ),
        Case::new(
            "red 132 moves 4 right, linear",
            displaced(bar(), red(132), across(128)),
            moved(4),
        ),
        Case::new(
            "red 124 moves 4 left",
            displaced(bar(), red(124), across(128)),
            moved(-4),
        ),
        Case::new(
            "a negative amount reverses",
            displaced(bar(), red(132), across(-128)),
            moved(-4),
        ),
        Case::new(
            "half a pixel blends the edges",
            displaced(bar(), red(129), across(64)),
            row_from_4([0, 0, 0, 0, 128, 255, 255, 255, 128, 0, 0, 0, 0, 0, 0, 0]),
        ),
        Case::new(
            "128 leaves the source in place",
            displaced(
                bar(),
                solid_map([128, 128, 128, 255], 0, 0),
                json!({ "amount_x": 500, "amount_y": 500 }),
            ),
            moved(0),
        ),
        Case::new(
            "a transparent map leaves the source in place",
            displaced(bar(), solid_map(CLEAR, 0, 0), across(128)),
            moved(0),
        ),
        Case::new(
            "green drives x when chosen",
            displaced(
                bar(),
                solid_map([128, 132, 0, 255], 0, 0),
                json!({ "amount_x": 128, "amount_y": 0, "channel_x": "green" }),
            ),
            moved(4),
        ),
        Case::new(
            "luma of grey 132 moves 4 right",
            displaced(
                bar(),
                solid_map([132, 132, 132, 255], 0, 0),
                json!({ "amount_x": 128, "amount_y": 0, "channel_x": "luma" }),
            ),
            moved(4),
        ),
        // The map covers columns 0 to 9, which read 4 columns to their right.
        Case::new(
            "only pixels under a smaller map move",
            displaced(bar(), solid_map([124, 0, 0, 255], 10, HEIGHT), across(128)),
            row_from_4([255, 255, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0]),
        ),
    ]);
}

#[test]
fn edges_clamp_or_read_transparent() {
    let red = json!({ "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255] } });
    let map = solid_map([128, 131, 0, 255], 0, 0);
    let edge = |edge: &str| json!({ "amount_x": 0, "amount_y": 128, "edge": edge });
    let red_at = |y| (5, y, [255, 0, 0, 255]);
    check(vec![
        Case::new(
            "clamp repeats the top row",
            displaced(red.clone(), map.clone(), edge("clamp")),
            Expect::Uniform([255, 0, 0, 255]),
        ),
        Case::new(
            "transparent reads nothing above the source",
            displaced(red, map, edge("transparent")),
            Expect::Probes(vec![(5, 0, CLEAR), (5, 2, CLEAR), red_at(3), red_at(7)]),
        ),
    ]);
}

/// Sources sized to their bounds, grown by the displacement, draw what full-frame sources draw.
#[test]
fn displacement_bounds_match_reference() {
    let canvas = (160, 120);
    let shape = json!({ "id": 1, "type": "shape", "params": {
        "position": [61.5, 40.25], "width": 23, "height": 17, "fill_paint": [255, 120, 0, 220] } });
    let noise = |width: u32, height: u32| {
        json!({ "id": 2, "type": "fractal_noise", "params": {
            "scale": 9, "octaves": 3, "contrast": 3, "width": width, "height": height,
            "evolution": "=frame / 5" } })
    };
    let row_of = |name: &str, source: Value, map: Value, params: Value, bounded: bool| {
        Case::new(
            name,
            scene(
                canvas,
                json!([source, map, { "id": 3, "type": "displacement_map",
                    "params": params, "in": { "source": 1, "map": 2 } }]),
            )
            .frames(3),
            Expect::Nonblank,
        )
        .frames([0.0, 1.0, 2.0])
        .bounded(bounded)
    };
    let row = |name: &str, map: Value, params: Value, bounded: bool| {
        row_of(name, shape.clone(), map, params, bounded)
    };
    // Clamping repeats the column at the canvas edge, which this shape covers.
    let at_edge = json!({ "id": 1, "type": "shape", "params": {
        "position": [-4, 70.5], "width": 19, "height": 13, "fill_paint": [40, 200, 90, 255] } });
    check(vec![
        row_of(
            "clamped at the canvas edge",
            at_edge,
            noise(0, 0),
            json!({ "amount_x": 25, "amount_y": 6 }),
            true,
        ),
        row(
            "noise map",
            noise(0, 0),
            json!({ "amount_x": 14.5, "amount_y": -9 }),
            true,
        ),
        row(
            "noise map, nearest and transparent",
            noise(0, 0),
            json!({ "amount_x": "=7 + frame", "amount_y": 20, "sampling": "nearest", "edge": "transparent" }),
            true,
        ),
        row(
            "a map smaller than the source's reach",
            noise(70, 50),
            json!({ "amount_x": 40, "amount_y": 40 }),
            true,
        ),
        row(
            "amounts past the canvas",
            noise(0, 0),
            json!({ "amount_x": 1e12, "amount_y": -1e12 }),
            false,
        ),
    ]);
}
