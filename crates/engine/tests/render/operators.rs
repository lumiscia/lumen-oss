//! Compositing and colour operators on known pixels. Every expected colour is worked out by hand
//! from the operator's definition, with translucent inputs so premultiplication matters. Normal
//! compositing works on linear light; blend modes and colour controls on sRGB-encoded colour.

use serde_json::{Value, json};

use crate::color::{encode, linear, over, premultiplied, rgba8, scaled, srgb8};
use crate::support::{Case, Expect, Frame, Scene, check, scene};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];
const BLACK: [u8; 4] = [0, 0, 0, 255];
/// A translucent, saturated colour whose straight and premultiplied forms differ.
const TEAL: [u8; 4] = [60, 120, 200, 128];

const WIDTH: u32 = 24;
const HEIGHT: u32 = 64;
const MIDDLE: u32 = HEIGHT / 2;

fn solid(id: u32, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "background", "params": { "paint": color, "width": 4, "height": 4 } })
}

/// A 4 × 4 `color` solid through `kind` with `params`.
fn through(color: [u8; 4], kind: &str, params: Value) -> Scene {
    scene(
        (4, 4),
        json!([solid(1, color),
            { "id": 2, "type": kind, "params": params, "in": { "source": 1 } }]),
    )
}

/// `layers` over a 4 × 4 opaque red base, merged by the node `kind`, a `merge` or a
/// `raster_multimerge`.
fn layered(kind: &str, params: Value, layers: &[[u8; 4]]) -> Scene {
    let mut nodes = vec![solid(1, [255, 0, 0, 255])];
    nodes.extend((2..).zip(layers).map(|(id, &color)| solid(id, color)));
    let ids: Vec<u32> = (1..=layers.len() as u32 + 1).collect();
    let inputs = match kind {
        "merge" => json!({ "base": 1, "overlay": 2 }),
        _ => json!({ "layers": ids }),
    };
    nodes.push(json!({ "id": 9, "type": kind, "params": params, "in": inputs }));
    scene((4, 4), json!(nodes))
}

/// A `width` × 64 white overlay merged over black, so the red channel of the merge is the matte.
/// `mask` is a list of nodes whose last one has id 5.
fn matte(width: u32, mask: Value, params: Value) -> Scene {
    let canvas = |id, color| {
        json!({ "id": id, "type": "background", "params": {
            "paint": color, "width": width, "height": HEIGHT } })
    };
    let mut nodes = vec![canvas(1, BLACK), canvas(2, WHITE)];
    nodes.extend(mask.as_array().unwrap().iter().cloned());
    nodes.push(json!({ "id": 3, "type": "merge", "params": params,
        "in": { "base": 1, "overlay": 2, "mask": 5 } }));
    scene((width, HEIGHT), json!(nodes))
}

/// A shape spanning the canvas height, half covering its first and last columns when `x` ends
/// in .5.
fn bar(id: u32, x: f64, width: u32, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "shape", "params": {
        "position": [x, 0], "width": width, "height": HEIGHT, "fill_paint": color } })
}

/// A mask covering no pixels of columns before 8, half of 8, all of 9 to 15 and half of 16.
fn rim(color: [u8; 4]) -> Value {
    json!([bar(5, 8.5, 8, color)])
}

fn solid_mask(color: [u8; 4], width: u32, height: u32) -> Value {
    json!([{ "id": 5, "type": "background", "params": {
        "paint": color, "width": width, "height": height } }])
}

/// The matte in columns 6 to 17 of the middle row, which hold the edges of `rim`. White over
/// black at matte `m` is `m` of the light.
fn matte_at(columns: [f64; 12]) -> Expect {
    let probes = (6..).zip(columns).map(|(x, m)| {
        let v = srgb8(m);
        (x, MIDDLE, [v, v, v, 255])
    });
    Expect::Probes(probes.collect())
}

/// The matte at `(x, y)` as a byte, from the light of white over black there.
fn matte_value(frame: &Frame, x: u32, y: u32) -> u8 {
    (linear(frame.pixel(x, y)[0]) * 255.0).round() as u8
}

/// The matte along the middle row.
fn matte_row(frame: &Frame) -> Vec<u8> {
    (0..frame.width)
        .map(|x| matte_value(frame, x, MIDDLE))
        .collect()
}

/// Boolean inputs: `a` covers columns 1 (half), 2 and 3, and 4 (half); `b` covers 3 (half), 4 to
/// 7, and 8 (half).
fn boolean(params: Value, b: u32) -> Scene {
    scene(
        (WIDTH, HEIGHT),
        json!([{ "id": 1, "type": "shape", "params": {
                "position": [1.5, 0], "width": 3, "height": HEIGHT, "fill_paint": WHITE } },
            { "id": 2, "type": "shape", "params": {
                "position": [3.5, 0], "width": 5, "height": HEIGHT, "fill_paint": WHITE } },
            { "id": 3, "type": "boolean", "params": params, "in": { "a": 1, "b": b } }]),
    )
}

/// The alpha of white in columns 0 to 9 of the middle row.
fn alpha_at(columns: [u8; 10]) -> Expect {
    let probes = (0..).zip(columns).map(|(x, v)| (x, MIDDLE, [v, v, v, v]));
    Expect::Probes(probes.collect())
}

#[test]
fn operators_produce_hand_computed_colors() {
    let lift = json!({ "exposure": 1, "offset": 0.5 });
    let green_shadow = json!({ "offset_x": 0, "offset_y": 0, "radius": 0,
        "color": [0, 255, 0, 255], "opacity": 1 });
    let level = json!({ "black_point": 0.1, "white_point": 0.9, "gamma": 1.2,
        "output_black": 0.05, "output_white": 0.95 });
    let lut = json!({ "lut": "rgb1d: 0,0,0; 1,0.5,0", "strength": 0.5, "interpolation": "linear" });
    let red = premultiplied([255, 0, 0, 255]);
    // Exposure scales linear light by `gain`, then applies offset and contrast about mid grey to
    // the encoded colour of TEAL, whose alpha is .502.
    let exposed = |gain: f64, contrast: f64, offset: f64| {
        let alpha = f64::from(TEAL[3]) / 255.0;
        let channel = |code: u8| {
            let encoded = (encode(linear(code) * gain) + offset - 0.5) * contrast + 0.5;
            (encoded.clamp(0.0, 1.0) * alpha * 255.0).round() as u8
        };
        [
            channel(TEAL[0]),
            channel(TEAL[1]),
            channel(TEAL[2]),
            TEAL[3],
        ]
    };
    // Screen works on encoded colour: each layer screens the running result to full green, then
    // full blue, which is composited in at alpha .251.
    let screened = srgb8(128.0 / 255.0 * 0.5);
    // Each scene is a 4 × 4 solid, and every pixel of the output is the colour.
    let rows = [
        // 50% blue at 50% opacity over red: (0, 0, .251) over (1, 0, 0) leaves .749 of the red.
        (
            "merge opacity",
            layered("merge", json!({ "opacity": 0.5 }), &[[0, 0, 255, 128]]),
            rgba8(over(scaled(premultiplied([0, 0, 255, 128]), 0.5), red)),
        ),
        // Screen of each layer onto the running result gives (1, 1/4, 1/4) of the light.
        (
            "multimerge of three layers with screen",
            layered(
                "raster_multimerge",
                json!({ "opacity": 0.5, "blend_mode": "screen" }),
                &[[0, 255, 0, 128], [0, 0, 255, 128]],
            ),
            [255, screened, screened, 255],
        ),
        // Half-red (.5, 0, 0, .5) over its own green shadow (0, .5, 0, .5), then over black.
        (
            "shadow of a translucent source",
            scene(
                (4, 4),
                json!([solid(1, BLACK), solid(2, [255, 0, 0, 128]),
                { "id": 3, "type": "shadow", "in": { "source": 2 }, "params": green_shadow },
                { "id": 4, "type": "merge", "in": { "base": 1, "overlay": 3 } }]),
            ),
            rgba8(over(
                over(
                    premultiplied([255, 0, 0, 128]),
                    premultiplied([0, 255, 0, 128]),
                ),
                premultiplied(BLACK),
            )),
        ),
        // The straight colour (.5, .5, .5) of a transparent pixel must not show through, and
        // the same lift of a translucent pixel does.
        (
            "exposure of a transparent pixel",
            through(CLEAR, "exposure", lift.clone()),
            CLEAR,
        ),
        (
            "exposure lifts a translucent pixel",
            through(TEAL, "exposure", lift.clone()),
            exposed(2.0, 1.0, 0.5),
        ),
        (
            "exposure gain, contrast and offset",
            through(
                TEAL,
                "exposure",
                json!({ "exposure": 0.5, "contrast": 1.2, "offset": -0.05 }),
            ),
            exposed(2f64.sqrt(), 1.2, -0.05),
        ),
        (
            "levels",
            through([64, 128, 200, 128], "levels", level),
            [35, 71, 107, 128],
        ),
        // The LUT keeps red, halves green and zeroes blue; half strength is the midpoint.
        (
            "color grade",
            through([200, 100, 60, 128], "color_grade", lut),
            [100, 38, 15, 128],
        ),
    ];
    let mut cases: Vec<_> = rows
        .into_iter()
        .map(|(name, scene, color)| Case::new(name, scene, Expect::Uniform(color)))
        .collect();
    // The hue turns a third of the wheel per frame, so frame 3 would repeat frame 0.
    let hue = json!({ "hue_degrees": "=frame * 120", "saturation": 0.5, "lightness": 0.1 });
    cases.push(
        Case::new(
            "hue and saturation follow the frame",
            through([200, 80, 40, 128], "hue_saturation", hue).frames(3),
            Expect::PerFrame(vec![
                Expect::Uniform([91, 64, 54, 128]),
                Expect::Uniform([54, 91, 64, 128]),
                Expect::Uniform([64, 54, 91, 128]),
            ]),
        )
        .frames([0.0, 1.0, 2.0]),
    );
    // The shape covers (4..8, 4..8) at alpha .784, so its blue shadow at opacity .5 has alpha
    // .392 and covers (7..11, 6..10).
    let shape = json!({ "id": 1, "type": "shape", "params": {
        "position": [4, 4], "width": 4, "height": 4,
        "fill_paint": [255, 100, 0, 200], "edge_antialias": false } });
    let offset = json!({ "offset_x": 3, "offset_y": 2, "radius": 0,
        "color": [0, 0, 255, 255], "opacity": 0.5 });
    cases.push(Case::new(
        "shadow offset places the shadow behind the shape",
        scene(
            (16, 16),
            json!([shape,
            { "id": 2, "type": "shadow", "params": offset, "in": { "source": 1 } }]),
        ),
        Expect::Probes(vec![
            (5, 5, [200, 78, 0, 200]),
            (9, 8, [0, 0, 100, 100]),
            (
                7,
                6,
                rgba8(over(
                    premultiplied([255, 100, 0, 200]),
                    scaled(premultiplied([0, 0, 255, 255]), 0.5 * 200.0 / 255.0),
                )),
            ),
            (10, 5, CLEAR),
            (3, 8, CLEAR),
            (6, 10, CLEAR),
        ]),
    ));
    check(cases);
}

/// An opaque `overlay` merged over an opaque `base` with `mode`.
fn blended(mode: &str, base: [u8; 3], overlay: [u8; 3]) -> Scene {
    let opaque = |[r, g, b]: [u8; 3]| [r, g, b, 255];
    scene(
        (4, 4),
        json!([solid(1, opaque(base)), solid(2, opaque(overlay)),
            { "id": 3, "type": "merge", "params": { "blend_mode": mode },
              "in": { "base": 1, "overlay": 2 } }]),
    )
}

#[test]
fn separable_blend_modes_match_the_w3c_formulas() {
    // Each row works the W3C Compositing formulas per channel on b = base / 255, s = overlay / 255.
    let rows = [
        // min(b + s, 1): red and green clip at 250 (200 + 50) and 250 (100 + 150), blue at 1.
        ("add", [200, 100, 50], [50, 150, 230], [250, 250, 255]),
        // |b - s|.
        ("difference", [200, 100, 50], [50, 150, 230], [150, 50, 180]),
        // b + s - 2bs.
        ("exclusion", [200, 100, 50], [50, 150, 230], [172, 132, 190]),
        // min(1, b / (1 - s)), with 0 for a black base and 1 for a white overlay.
        ("color_dodge", [0, 100, 200], [128, 255, 128], [0, 255, 255]),
        // 1 - min(1, (1 - b) / s), with 1 for a white base and 0 for a black overlay.
        ("color_burn", [255, 100, 200], [128, 0, 128], [255, 0, 145]),
        // Unclipped dodge, the black-base rule winning over a white overlay, and unclipped dodge.
        ("color_dodge", [64, 0, 30], [100, 255, 200], [105, 0, 139]),
        // Unclipped burn, the white-base rule winning over a black overlay, and burn clipping to 0.
        ("color_burn", [200, 255, 60], [230, 0, 100], [194, 255, 0]),
        // Multiply by 2s for s <= .5, else screen with 2s - 1.
        (
            "hard_light",
            [200, 100, 50],
            [64, 200, 230],
            [100, 188, 215],
        ),
        // b - (1 - 2s) b (1 - b) for s <= .5, else b + (2s - 1) (D(b) - b), where D is the
        // cubic for b <= .25 and the square root above it. The channels cover all three branches.
        ("soft_light", [40, 128, 30], [100, 200, 220], [33, 158, 69]),
    ];
    check(
        rows.into_iter()
            .map(|(mode, base, overlay, [r, g, b])| {
                Case::new(
                    mode,
                    blended(mode, base, overlay),
                    Expect::Uniform([r, g, b, 255]),
                )
            })
            .collect(),
    );
}

#[test]
fn nonseparable_blend_modes_match_the_w3c_formulas() {
    // Each row works the W3C Compositing Lum, SetLum, Sat and SetSat helpers on
    // b = base / 255, s = overlay / 255, with Lum = .3r + .59g + .11b.
    let rows = [
        // The overlay's hue and the base's saturation and luminosity.
        ("hue", [200, 100, 50], [50, 150, 230], [59, 142, 209]),
        // The base's hue and luminosity and the overlay's saturation.
        ("saturation", [200, 100, 50], [50, 150, 230], [215, 95, 35]),
        // The overlay's hue and saturation and the base's luminosity.
        ("color", [200, 100, 50], [50, 150, 230], [46, 146, 226]),
        // The base's hue and saturation and the overlay's luminosity.
        ("luminosity", [200, 100, 50], [50, 150, 230], [204, 104, 54]),
        // ClipColor rescales around the luminosity when a channel falls below 0 (hue, color)
        ("hue", [30, 60, 200], [240, 200, 20], [85, 69, 0]),
        ("saturation", [30, 60, 200], [240, 200, 20], [19, 58, 239]),
        ("color", [30, 60, 200], [240, 200, 20], [85, 69, 0]),
        // or rises above 1 (luminosity).
        ("luminosity", [30, 60, 200], [240, 200, 20], [175, 189, 255]),
        // Above 1 (hue) and below 0 (luminosity) again.
        ("hue", [20, 230, 40], [250, 30, 30], [255, 99, 99]),
        ("luminosity", [20, 230, 40], [250, 30, 30], [0, 160, 15]),
    ];
    let mut cases: Vec<Case> = rows
        .into_iter()
        .map(|(mode, base, overlay, [r, g, b])| {
            Case::new(
                mode,
                blended(mode, base, overlay),
                Expect::Uniform([r, g, b, 255]),
            )
        })
        .collect();
    // A grey base under black lands on a uniform result whose ClipColor denominator rounds to
    // zero. A clear overlay keeps the base, and a half-transparent one halves its light.
    let halved = srgb8((1.0 - 128.0 / 255.0) * linear(193));
    for (alpha, expected) in [(0, 193), (128, halved)] {
        cases.push(Case::new(
            "luminosity of a translucent black over grey",
            scene(
                (4, 4),
                json!([solid(1, [193, 193, 193, 255]), solid(2, [0, 0, 0, alpha]),
                    { "id": 3, "type": "merge", "params": { "blend_mode": "luminosity" },
                      "in": { "base": 1, "overlay": 2 } }]),
            ),
            Expect::Uniform([expected, expected, expected, 255]),
        ));
    }
    check(cases);
}

#[test]
fn mattes_select_by_alpha_or_luma() {
    // Columns 6 to 17 of a mask that covers half of 8, all of 9 to 15 and half of 16.
    let edge = |full: f64| {
        let half = full / 2.0;
        [
            0.0, 0.0, half, full, full, full, full, full, full, full, half, 0.0,
        ]
    };
    let inverted = edge(1.0).map(|matte| 1.0 - matte);
    let luma = |color: [u8; 4]| {
        let [r, g, b, _] = premultiplied(color);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };
    let mut cases = vec![
        Case::new(
            "alpha keeps antialiased edges",
            matte(WIDTH, rim(WHITE), json!({})),
            matte_at(edge(1.0)),
        ),
        // Luma is the Rec. 709 luminance of linear light over black: .2126 for full red, and half
        // that on a half-covered edge.
        Case::new(
            "luma of opaque red",
            matte(WIDTH, rim([255, 0, 0, 255]), json!({ "mask_mode": "luma" })),
            matte_at(edge(0.2126)),
        ),
        Case::new(
            "alpha inverted",
            matte(WIDTH, rim(WHITE), json!({ "invert": true })),
            matte_at(inverted),
        ),
        Case::new(
            "luma inverted",
            matte(
                WIDTH,
                rim(WHITE),
                json!({ "mask_mode": "luma", "invert": true }),
            ),
            matte_at(inverted),
        ),
        Case::new(
            "luma of an opaque green solid",
            matte(
                WIDTH,
                solid_mask([0, 255, 0, 255], WIDTH, HEIGHT),
                json!({ "mask_mode": "luma" }),
            ),
            matte_at([0.7152; 12]),
        ),
    ];
    // A translucent orange (200, 90, 40) at alpha .588, whose luma over black is that of its
    // premultiplied light. A zero feather expression must read the mask as no feather does.
    let orange = [200, 90, 40, 150];
    for (mode, full) in [("alpha", 150.0 / 255.0), ("luma", luma(orange))] {
        let translucent = |feather: Value| {
            let params = json!({ "mask_mode": mode, "feather": feather });
            matte(WIDTH, rim(orange), params)
        };
        cases.push(Case::new(
            format!("{mode} of a translucent mask"),
            translucent(json!(0)),
            matte_at(edge(full)),
        ));
        cases.push(
            Case::new(
                format!("{mode} with a zero feather expression"),
                translucent(json!("=0")),
                Expect::Matches(translucent(json!(0)).into()),
            )
            .tolerance(0),
        );
    }
    // Sent through the blur's float texture, this gray's luma rounds to 160 instead of 161.
    let gray = |feather: Value| {
        matte(
            WIDTH,
            rim([178, 178, 178, 230]),
            json!({ "mask_mode": "luma", "feather": feather }),
        )
    };
    cases.push(
        Case::new(
            "a zero feather expression does not resample a gray mask",
            gray(json!("=0")),
            Expect::Matches(gray(json!(0)).into()),
        )
        .tolerance(0),
    );
    check(cases);
}

#[test]
fn feather_blurs_the_mask_before_inverting() {
    let feathered = |mask, feather: Value| matte(WIDTH, mask, json!({ "feather": feather }));
    // A blurred step is point-symmetric about its edge, wherever the kernel falls: the value k
    // columns inside plus the value k columns outside is full coverage.
    let symmetric = |row: &[u8], inside: usize, outside: usize| {
        (row[inside] as i32 + row[outside] as i32 - 255).abs() <= 3
    };
    let rising = |row: &[u8], from, to| row[from..=to].windows(2).all(|pair| pair[0] <= pair[1]);
    check(vec![
        Case::new(
            "interior edge spreads both ways and keeps its coverage",
            feathered(rim(WHITE), json!(2)),
            Expect::Custom(Box::new(move |frames| {
                let row = matte_row(&frames[0]);
                // Eight columns of full coverage: the blur redistributes it.
                let total: i32 = row.iter().map(|&v| i32::from(v)).sum();
                assert!(
                    row[..3] == [0; 3]
                        && row[12] >= 250
                        && row[8].abs_diff(128) <= 2
                        && (1..=3).all(|k| symmetric(&row, 8 + k, 8 - k))
                        && row[7] > 0
                        && row[9] < 255
                        && rising(&row, 6, 11)
                        && (total - 2040).abs() <= 100,
                    "{row:?}"
                );
            })),
        ),
        Case::new(
            "texture edge of a smaller mask spreads past it",
            feathered(solid_mask(WHITE, 12, HEIGHT), json!(2)),
            Expect::Custom(Box::new(move |frames| {
                let row = matte_row(&frames[0]);
                assert!(
                    symmetric(&row, 11, 12)
                        && row[12] > 40
                        && row[11] < 230
                        && row[6..=16].windows(2).all(|pair| pair[0] >= pair[1]),
                    "{row:?}"
                );
            })),
        ),
        Case::new(
            "a mask larger than the canvas blurs with what lies past its edge",
            feathered(solid_mask(WHITE, WIDTH + 40, HEIGHT + 30), json!(2)),
            Expect::Custom(Box::new(|frames| {
                // The mask continues past the right and bottom edges but starts at the left.
                let pixel = |x, y| matte_value(&frames[0], x, y);
                assert!(
                    pixel(WIDTH - 1, MIDDLE) >= 254
                        && pixel(12, HEIGHT - 1) >= 254
                        && pixel(0, MIDDLE) < 230,
                    "right {}, bottom {}, left {}",
                    pixel(WIDTH - 1, MIDDLE),
                    pixel(12, HEIGHT - 1),
                    pixel(0, MIDDLE)
                );
            })),
        ),
        Case::new(
            "a mask the size of the canvas fades at the canvas edge",
            feathered(solid_mask(WHITE, WIDTH, HEIGHT), json!(2)),
            Expect::Custom(Box::new(|frames| {
                assert!(
                    matte_value(&frames[0], WIDTH - 1, MIDDLE) < 230,
                    "{:?}",
                    matte_row(&frames[0])
                );
            })),
        ),
        // Reduced-resolution filtering must cover the mask's whole extent on both axes.
        Case::new(
            "a wide feather of a square mask falls off evenly on both axes",
            feathered(solid_mask(WHITE, 64, 64), json!(64)),
            Expect::Custom(Box::new(|frames| {
                let (right, bottom) = (
                    matte_value(&frames[0], 23, 12),
                    matte_value(&frames[0], 12, 23),
                );
                assert!(
                    right > 0 && right.abs_diff(bottom) <= 2,
                    "{right}, {bottom}"
                );
            })),
        ),
        Case::new(
            "negative feather is none",
            feathered(rim(WHITE), json!(-3)),
            matte_at([0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.5, 0.0]),
        ),
        Case::new(
            "huge feather washes the mask out to nothing",
            feathered(rim(WHITE), json!(1.0e9)),
            Expect::Custom(Box::new(|frames| {
                let frame = &frames[0];
                let washed_out = (0..frame.height)
                    .all(|y| (0..frame.width).all(|x| matte_value(frame, x, y) == 0));
                assert!(washed_out, "{:?}", matte_row(frame));
            })),
        ),
        Case::new(
            "an expression sweep changes smoothly from the sharp edge",
            feathered(rim(WHITE), json!("=frame * 0.25")).frames(33),
            Expect::Custom(Box::new(|frames| {
                let rows: Vec<_> = frames.iter().map(matte_row).collect();
                let step = |pair: &[Vec<u8>]| {
                    pair[0]
                        .iter()
                        .zip(&pair[1])
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap()
                };
                assert!(
                    rows[0][7] == 0 && rows[0][8].abs_diff(128) <= 1 && rows[32][7] > 0,
                    "{:?} -> {:?}",
                    rows[0],
                    rows[32]
                );
                for (frame, pair) in rows.windows(2).enumerate() {
                    assert!(
                        step(pair) <= 16,
                        "frame {frame} -> {}: {:?} -> {:?}",
                        frame + 1,
                        pair[0],
                        pair[1]
                    );
                }
            })),
        )
        .frames((0..33).map(f64::from)),
    ]);
}

#[test]
fn a_large_feather_has_no_ripples_on_a_thin_line() {
    // A 1-pixel line at column 128 of 256 spreads into a wide, low bump that only falls away.
    check(vec![Case::new(
        "thin line",
        matte(
            256,
            json!([bar(5, 128.0, 1, WHITE)]),
            json!({ "feather": 32 }),
        ),
        Expect::Custom(Box::new(|frames| {
            let row = matte_row(&frames[0]);
            let (left, right) = (&row[..=128], &row[128..]);
            assert!(
                (3..=9).contains(&row[128])
                    && row[104] > 0
                    && row[152] > 0
                    && left.windows(2).all(|pair| pair[1] + 1 >= pair[0])
                    && right.windows(2).all(|pair| pair[0] + 1 >= pair[1]),
                "{row:?}"
            );
        })),
    )]);
}

#[test]
fn boolean_combines_antialiased_coverage() {
    let xor = json!({ "operation": "xor" });
    let subtract = json!({ "operation": "subtract" });
    // Columns 0 to 9. `a` covers half of 1, all of 2 and 3, and half of 4. `b` covers half of 3,
    // all of 4 to 7, and half of 8. The second input is `b`, or `a` again with `2` replaced by 1.
    let rows = [
        (
            "union",
            json!({}),
            2,
            [0, 128, 255, 255, 255, 255, 255, 255, 128, 0],
        ),
        (
            "intersect",
            json!({ "operation": "intersect" }),
            2,
            [0, 0, 0, 128, 128, 0, 0, 0, 0, 0],
        ),
        (
            "subtract",
            subtract.clone(),
            2,
            [0, 128, 255, 128, 0, 0, 0, 0, 0, 0],
        ),
        (
            "xor on a partial overlap",
            xor,
            2,
            [0, 128, 255, 128, 128, 255, 255, 255, 128, 0],
        ),
        ("subtracting a mask from itself", subtract, 1, [0; 10]),
        // Coverage above the cutoff becomes opaque.
        (
            "threshold below the coverage",
            json!({ "threshold": 0.25 }),
            2,
            [0, 255, 255, 255, 255, 255, 255, 255, 255, 0],
        ),
        // Coverage at or below the cutoff passes through unchanged.
        (
            "threshold above the coverage",
            json!({ "threshold": 0.75 }),
            2,
            [0, 128, 255, 255, 255, 255, 255, 255, 128, 0],
        ),
    ];
    check(
        rows.into_iter()
            .map(|(name, params, b, columns)| {
                Case::new(name, boolean(params, b), alpha_at(columns))
            })
            .collect(),
    );
}

/// A small offset shape is stored around its own bounds, so these read it at an offset.
#[test]
fn color_grade_and_crop_read_a_small_offset_layer() {
    let on_canvas = |effect: Value| {
        scene(
            (384, 288),
            json!([
                { "id": 1, "type": "shape", "params": {
                    "position": [100, 50], "width": 14, "height": 11,
                    "fill_paint": [200, 100, 0, 255], "edge_antialias": false
                } },
                { "id": 2, "in": { "source": 1 }, "type": effect["type"], "params": effect["params"] },
            ]),
        )
    };
    let lut = json!({ "type": "color_grade", "params": {
        "lut": "rgb1d: 0,0,0; 1,0.5,0", "interpolation": "linear"
    } });
    let crop = |x, y, width, height| json!({ "type": "crop", "params": { "x": x, "y": y, "width": width, "height": height } });
    let graded = [200, 50, 0, 255];
    let orange = [200, 100, 0, 255];
    check(vec![
        Case::new(
            "color grade grades the shape and leaves the rest clear",
            on_canvas(lut),
            Expect::Probes(vec![
                (100, 50, graded),
                (113, 60, graded),
                (99, 50, CLEAR),
                (114, 60, CLEAR),
                (0, 0, CLEAR),
                (383, 287, CLEAR),
            ]),
        )
        .tolerance(1),
        Case::new(
            "crop keeps the part of the shape inside its region",
            on_canvas(crop(105, 52, 4, 3)),
            Expect::Probes(vec![
                (105, 52, orange),
                (108, 54, orange),
                (104, 52, CLEAR),
                (109, 52, CLEAR),
                (105, 51, CLEAR),
                (105, 55, CLEAR),
                (100, 50, CLEAR),
            ]),
        ),
        Case::new(
            "crop of a region that overhangs the shape",
            on_canvas(crop(110, 55, 30, 30)),
            Expect::Probes(vec![
                (110, 55, orange),
                (113, 60, orange),
                (114, 60, CLEAR),
                (113, 61, CLEAR),
                (109, 55, CLEAR),
                (130, 70, CLEAR),
            ]),
        ),
        Case::new(
            "crop of a region the shape doesn't reach",
            on_canvas(crop(200, 100, 20, 20)),
            Expect::Probes(vec![(205, 105, CLEAR), (100, 50, CLEAR)]),
        ),
    ]);
}
