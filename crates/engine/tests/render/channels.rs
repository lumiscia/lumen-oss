//! `channels`: each pick on flat colour, with expected values worked out from the straight colour.

use serde_json::{Value, json};

use crate::color::{linear, opaque, premultiplied, rgba8};
use crate::support::{Case, Expect, Scene, check, scene};

/// `channels` over a flat `paint`, with a flat `other` when it is given.
fn channels(paint: [u8; 4], other: Option<[u8; 4]>, params: Value) -> Scene {
    let mut nodes = vec![json!({ "id": 1, "type": "background", "params": { "paint": paint } })];
    let mut inputs = json!({ "source": 1 });
    if let Some(other) = other {
        nodes.push(json!({ "id": 2, "type": "background", "params": { "paint": other } }));
        inputs["other"] = json!(2);
    }
    nodes.push(json!({ "id": 3, "type": "channels", "in": inputs, "params": params }));
    scene((4, 4), Value::Array(nodes))
}

#[test]
fn defaults_pass_the_source_through() {
    let paint = [200, 80, 40, 128];
    check(vec![Case::new(
        "default params",
        channels(paint, None, json!({})),
        Expect::Uniform(rgba8(premultiplied(paint))),
    )]);
}

#[test]
fn alpha_from_the_luma_of_another_raster() {
    // A grey of code 128 has that linear value as its luma, which is the alpha.
    let alpha = linear(128);
    let red = [linear(255), 0.0, 0.0];
    check(vec![Case::new(
        "other_luma as alpha",
        channels(
            [255, 0, 0, 255],
            Some([128, 128, 128, 255]),
            json!({ "alpha": "other_luma" }),
        ),
        Expect::Uniform(rgba8([
            red[0] * alpha,
            red[1] * alpha,
            red[2] * alpha,
            alpha,
        ])),
    )]);
}

#[test]
fn channels_swap() {
    check(vec![Case::new(
        "red and blue swapped",
        channels(
            [200, 80, 40, 255],
            None,
            json!({ "red": "source_blue", "blue": "source_red" }),
        ),
        Expect::Uniform(opaque([linear(40), linear(80), linear(200)])),
    )]);
}
