//! Compositing and colour operators on known pixels. Every expected colour is worked out by hand
//! from the operator's definition, with translucent inputs so premultiplication matters.

use lumen_engine::{composition::Composition, gpu::CompileContext};
use lumen_gpu::PassDesc;
use serde_json::{Value, json};

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

/// Converts the premultiplied raster `source` to straight alpha.
fn straight(id: u32, source: u32) -> Value {
    json!({ "id": id, "type": "alpha_premultiply", "in": { "source": source },
        "params": { "mode": "unpremultiply" } })
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

/// The same coverage as `rim`, in a straight-alpha raster.
fn straight_rim(color: [u8; 4]) -> Value {
    json!([bar(6, 8.5, 8, color), straight(5, 6)])
}

fn solid_mask(color: [u8; 4], width: u32, height: u32) -> Value {
    json!([{ "id": 5, "type": "background", "params": {
        "paint": color, "width": width, "height": height } }])
}

/// The matte in columns 6 to 17 of the middle row, which hold the edges of `rim`.
fn matte_at(columns: [u8; 12]) -> Expect {
    let probes = (6..).zip(columns).map(|(x, v)| (x, MIDDLE, [v, v, v, 255]));
    Expect::Probes(probes.collect())
}

/// The red channel of the middle row.
fn matte_row(frame: &Frame) -> Vec<u8> {
    (0..frame.width)
        .map(|x| frame.pixel(x, MIDDLE)[0])
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
    let swap = json!({ "red": "blue", "blue": "red", "alpha": "one" });
    let green_shadow = json!({ "offset_x": 0, "offset_y": 0, "radius": 0,
        "color": [0, 255, 0, 255], "opacity": 1 });
    let level = json!({ "black_point": 0.1, "white_point": 0.9, "gamma": 1.2,
        "output_black": 0.05, "output_white": 0.95 });
    let lut =
        json!({ "lut_source": "rgb1d: 0,0,0; 255,128,0", "strength": 0.5, "interpolation": 1 });
    // Each scene is a 4 × 4 solid, and every pixel of the output is the colour.
    let rows = [
        // 50% blue at 50% opacity over red: (0, 0, .251) over (1, 0, 0) leaves .749 of the red.
        (
            "merge opacity",
            layered("merge", json!({ "opacity": 0.5 }), &[[0, 0, 255, 128]]),
            [191, 0, 64, 255],
        ),
        // Screen of each layer onto the running result gives (1, 1/4, 1/4); normal blending
        // would give (143, 48, 64).
        (
            "multimerge of three layers with screen",
            layered(
                "raster_multimerge",
                json!({ "opacity": 0.5, "blend_mode": "screen" }),
                &[[0, 255, 0, 128], [0, 0, 255, 128]],
            ),
            [255, 64, 64, 255],
        ),
        (
            "straight overlay is premultiplied before merging",
            scene(
                (4, 4),
                json!([solid(1, BLACK), solid(2, [255, 0, 0, 128]), straight(3, 2),
                { "id": 4, "type": "merge", "in": { "base": 1, "overlay": 3 } }]),
            ),
            [128, 0, 0, 255],
        ),
        (
            "wgsl shader premultiplies a straight input",
            scene(
                (4, 4),
                json!([solid(1, [200, 60, 0, 128]), straight(2, 1),
                { "id": 3, "type": "wgsl_shader", "in": { "source": [2] } }]),
            ),
            [100, 30, 0, 128],
        ),
        // Half-red (.5, 0, 0, .5) over its own green shadow (0, .5, 0, .5), then over black.
        (
            "shadow composites a straight source in premultiplied space",
            scene(
                (4, 4),
                json!([solid(1, BLACK), solid(2, [255, 0, 0, 128]), straight(3, 2),
                { "id": 4, "type": "shadow", "in": { "source": 3 }, "params": green_shadow },
                { "id": 5, "type": "merge", "in": { "base": 1, "overlay": 4 } }]),
            ),
            [128, 64, 0, 255],
        ),
        // The straight colour (.5, .5, .5) of a transparent pixel must not show through, and
        // the same lift of a translucent pixel does: 2 * .235 + .5 = .97 at alpha .502.
        (
            "exposure of a transparent pixel",
            through(CLEAR, "exposure", lift.clone()),
            CLEAR,
        ),
        (
            "exposure lifts a translucent pixel",
            through(TEAL, "exposure", lift.clone()),
            [124, 128, 128, 128],
        ),
        // A straight raster stays straight: (.235, .471, .784) lifted to (.97, 1, 1), unconverted.
        (
            "exposure of a straight raster",
            scene(
                (4, 4),
                json!([solid(1, TEAL), straight(2, 1),
                { "id": 3, "type": "exposure", "params": lift, "in": { "source": 2 } }]),
            ),
            [247, 255, 255, 128],
        ),
        // Gain 2^.5 and contrast 1.2 about the midpoint: red .239, green .639, blue clamps at 1.
        (
            "exposure gain, contrast and offset",
            through(
                TEAL,
                "exposure",
                json!({ "exposure": 0.5, "contrast": 1.2, "offset": -0.05 }),
            ),
            [30, 81, 128, 128],
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
        // Premultiplied (32, 64, 128, 128) with red and blue swapped and alpha forced to 1.
        (
            "channel shuffle",
            through([64, 128, 255, 128], "channel_shuffle", swap),
            [128, 64, 32, 255],
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
            (7, 6, [200, 78, 22, 222]),
            (10, 5, CLEAR),
            (3, 8, CLEAR),
            (6, 10, CLEAR),
        ]),
    ));
    check(cases);
}

#[test]
fn mattes_select_by_alpha_or_luma() {
    // Columns 6 to 17 of a mask that covers half of 8, all of 9 to 15 and half of 16.
    let edge = |full: u8| {
        let half = full / 2;
        [
            0, 0, half, full, full, full, full, full, full, full, half, 0,
        ]
    };
    let inverted = [255, 255, 127, 0, 0, 0, 0, 0, 0, 0, 127, 255];
    let mut cases = vec![
        Case::new(
            "alpha keeps antialiased edges",
            matte(WIDTH, rim(WHITE), json!({})),
            matte_at(edge(255)),
        ),
        // Luma is Rec. 709 over black: .2126 of full red is 54, and 27 on a half-covered edge.
        Case::new(
            "luma of opaque red",
            matte(WIDTH, rim([255, 0, 0, 255]), json!({ "mask_mode": "luma" })),
            matte_at(edge(54)),
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
            matte_at([182; 12]),
        ),
    ];
    // A translucent orange (200, 90, 40) at alpha .588: its alpha is 150, and its luma over
    // black is .2126 * 117.6 + .7152 * 52.9 + .0722 * 23.5 = 65. A zero feather expression must
    // read the mask as no feather does, and a straight mask must feather like a premultiplied one.
    let orange = [200, 90, 40, 150];
    for (mode, full) in [("alpha", 150), ("luma", 65)] {
        let straight = |feather: Value| {
            let params = json!({ "mask_mode": mode, "feather": feather });
            matte(WIDTH, straight_rim(orange), params)
        };
        let premultiplied = matte(
            WIDTH,
            rim(orange),
            json!({ "mask_mode": mode, "feather": 2 }),
        );
        cases.push(Case::new(
            format!("{mode} of a translucent straight mask"),
            straight(json!(0)),
            matte_at(edge(full)),
        ));
        cases.push(
            Case::new(
                format!("{mode} with a zero feather expression"),
                straight(json!("=0")),
                Expect::Matches(straight(json!(0)).into()),
            )
            .tolerance(0),
        );
        cases.push(
            Case::new(
                format!("{mode} of a straight mask feathers like a premultiplied one"),
                straight(json!(2)),
                Expect::Matches(premultiplied.into()),
            )
            .tolerance(2),
        );
    }
    // Sent through the blur's float texture, this gray's luma rounds to 160 instead of 161.
    let gray = |feather: Value| {
        matte(
            WIDTH,
            straight_rim([178, 178, 178, 230]),
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
                let pixel = |x, y| frames[0].pixel(x, y)[0];
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
                    frames[0].pixel(WIDTH - 1, MIDDLE)[0] < 230,
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
                let (right, bottom) = (frames[0].pixel(23, 12)[0], frames[0].pixel(12, 23)[0]);
                assert!(
                    right > 0 && right.abs_diff(bottom) <= 2,
                    "{right}, {bottom}"
                );
            })),
        ),
        Case::new(
            "negative feather is none",
            feathered(rim(WHITE), json!(-3)),
            matte_at([0, 0, 128, 255, 255, 255, 255, 255, 255, 255, 128, 0]),
        ),
        Case::new(
            "huge feather washes the mask out to nothing",
            feathered(rim(WHITE), json!(1.0e9)),
            Expect::Uniform(BLACK),
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

#[test]
fn a_feather_that_is_never_positive_compiles_no_blur_passes() {
    let blur_passes = |feather: Value| {
        let composition: Composition =
            matte(WIDTH, rim(WHITE), json!({ "feather": feather })).into();
        let compiled = CompileContext::new(&composition).compile().unwrap();
        let is_blur = |pass: &PassDesc| {
            matches!(pass, PassDesc::Compute(desc)
                if desc.label.as_deref().is_some_and(|label| label.starts_with("merge-feather:")))
        };
        compiled
            .plan
            .passes()
            .iter()
            .filter(|pass| is_blur(&pass.desc))
            .count()
    };
    assert_eq!((blur_passes(json!(0)), blur_passes(json!(-2))), (0, 0));
    assert!(blur_passes(json!(2)) > 0 && blur_passes(json!("=0")) > 0);
}
