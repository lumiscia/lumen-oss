//! Gaussian blur and shadow: the kernel's response and its continuity across the levels it is
//! computed on, and sources sized to their bounds drawing what a full-frame render draws.

use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, gpu, render, scene};

const INTER: &[u8] = include_bytes!("../../../text/assets/inter/InterVariable-Latin.ttf");

fn white(x: f64, y: f64, width: u32, height: u32) -> Value {
    json!({ "type": "shape", "params": {
        "position": [x, y], "width": width, "height": height, "fill_paint": [255, 255, 255, 255]
    } })
}

/// A 14 × 11 translucent orange shape, whose `position` may be an expression.
fn small(position: Value) -> Value {
    json!({ "type": "shape", "params": {
        "position": position, "width": 14, "height": 11, "fill_paint": [255, 100, 0, 200]
    } })
}

fn blur(radius: impl Into<Value>) -> Value {
    json!({ "type": "blur", "params": { "radius": radius.into() } })
}

fn shadow_in(color: [u8; 4], opacity: f64, offset: [Value; 2], radius: impl Into<Value>) -> Value {
    let [offset_x, offset_y] = offset;
    json!({ "type": "shadow", "params": {
        "offset_x": offset_x, "offset_y": offset_y, "radius": radius.into(),
        "color": color, "opacity": opacity
    } })
}

/// A black shadow at full opacity, so its alpha outside the source is the blurred alpha.
fn shadow(offset: [f64; 2], radius: impl Into<Value>) -> Value {
    shadow_in([0, 0, 0, 255], 1.0, offset.map(Value::from), radius)
}

fn tinted_shadow(offset: [Value; 2], radius: impl Into<Value>) -> Value {
    shadow_in([20, 40, 200, 255], 0.8, offset, radius)
}

fn unpremultiply() -> Value {
    json!({ "type": "alpha_premultiply", "params": { "mode": "unpremultiply" } })
}

/// `nodes` as a line, each fed by the one before.
fn chain(canvas: (u32, u32), nodes: Vec<Value>) -> Scene {
    let nodes = nodes.into_iter().enumerate().map(|(index, mut node)| {
        node["id"] = json!(index + 1);
        if index > 0 {
            node["in"] = json!({ "source": index });
        }
        node
    });
    scene(canvas, Value::Array(nodes.collect()))
}

/// An expression that is `values[n]` at frame `n`, for a scene of that many frames.
fn by_frame(values: &[f64]) -> Value {
    let (last, rest) = values.split_last().unwrap();
    let opened: String = (0..rest.len())
        .map(|frame| format!("if(frame < {frame}.5, {}, ", rest[frame]))
        .collect();
    json!(format!("={opened}{last}{}", ")".repeat(rest.len())))
}

fn frame_range(count: usize) -> impl Iterator<Item = f64> {
    (0..count).map(|frame| frame as f64)
}

/// A scene whose effect takes `values[n]` at frame `n`, and a row that renders each of them.
fn swept(
    name: String,
    canvas: (u32, u32),
    nodes: Vec<Value>,
    count: usize,
    expect: Expect,
) -> Case {
    Case::new(name, chain(canvas, nodes).frames(count as u32), expect).frames(frame_range(count))
}

fn check(cases: impl IntoIterator<Item = Case>) {
    crate::support::check(cases.into_iter().collect());
}

fn custom(check: impl Fn(&[Frame]) + 'static) -> Expect {
    Expect::Custom(Box::new(check))
}

fn alpha(frame: &Frame, x: u32, y: u32) -> f64 {
    f64::from(frame.pixel(x, y)[3])
}

fn total_alpha(frame: &Frame) -> f64 {
    (frame.rgba.as_chunks::<4>().0.iter())
        .map(|pixel| f64::from(pixel[3]))
        .sum()
}

/// Asserts that every frame has alpha above `floor` at every probe, and that the two frames of
/// each pair differ by at most `step` there.
fn continuous(probes: Vec<(u32, u32)>, floor: f64, step: f64, pairs: Vec<[usize; 2]>) -> Expect {
    custom(move |frames| {
        for frame in frames {
            for &(x, y) in &probes {
                let value = alpha(frame, x, y);
                assert!(
                    value > floor,
                    "frame {}: alpha at ({x}, {y}) is {value}, expected above {floor}",
                    frame.at
                );
            }
        }
        for &[before, after] in &pairs {
            for &(x, y) in &probes {
                let (a, b) = (alpha(&frames[before], x, y), alpha(&frames[after], x, y));
                assert!(
                    (a - b).abs() <= step,
                    "frames {before} -> {after}: alpha at ({x}, {y}) jumps from {a} to {b}"
                );
            }
        }
    })
}

/// Asserts that frame 1 differs from frame 0 by at most `step` at every byte, and that frame 0
/// has coverage.
fn no_pop(step: u8) -> Expect {
    custom(move |frames| {
        assert!(total_alpha(&frames[0]) > 1000.0, "frame 0 is nearly blank");
        let worst = (frames[0].rgba.iter().zip(&frames[1].rgba))
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= step, "the output jumps by {worst}");
    })
}

#[test]
fn gaussian_response() {
    // A (2r + 1)² box blur has variance r(r + 1) / 3 per axis and spreads a pixel to 1 / (2r + 1)².
    let impulse = |radius: f64| {
        let nodes = vec![white(16.0, 16.0, 1, 1), blur(radius)];
        Case::new(
            format!("radius {radius} spreads a pixel like the box blur it replaced"),
            chain((32, 32), nodes),
            custom(move |frames| {
                let frame = &frames[0];
                let (mut mass, mut spread) = (0.0, 0.0);
                for x in 0..32 {
                    let value = alpha(frame, x, 16);
                    mass += value;
                    spread += value * (f64::from(x) - 16.0).powi(2);
                }
                let box_variance = radius * (radius + 1.0) / 3.0;
                let box_center = 255.0 / (2.0 * radius + 1.0).powi(2);
                let (variance, center) = (spread / mass, alpha(frame, 16, 16));
                assert!(
                    (variance - box_variance).abs() <= 0.15 * box_variance,
                    "variance {variance}, box blur's {box_variance}"
                );
                assert!(
                    (total_alpha(frame) - 255.0).abs() <= 8.0,
                    "total alpha {}",
                    total_alpha(frame)
                );
                assert!(
                    center > box_center && center < 3.0 * box_center,
                    "center alpha {center}, box blur's {box_center}"
                );
            }),
        )
    };
    // 60 texels beyond the edge is outside a 32 texel reach. A blur dilutes the square and keeps
    // its energy; a shadow draws the square over its blur.
    let wide = |effect: Value, name: &str, blurred: bool| {
        Case::new(
            name,
            chain((256, 256), vec![white(112.0, 112.0, 32, 32), effect]),
            custom(move |frames| {
                let frame = &frames[0];
                let far = alpha(frame, 52, 128);
                assert!(far > 0.0, "radius 80 does not reach 60 pixels: {far}");
                if blurred {
                    let (center, total) = (alpha(frame, 128, 128), total_alpha(frame) / 255.0);
                    assert!(center > 0.0 && center < 128.0, "center alpha {center}");
                    assert!(
                        (total - 1024.0).abs() < 0.03 * 1024.0,
                        "total alpha {total}"
                    );
                }
            }),
        )
    };
    // The fringe of a half-covered edge stays white over transparent, whatever the source's alpha.
    let fringe = |kind: &str, fill: [u8; 4], straight: bool| {
        let mut source = white(24.5, 24.5, 16, 16);
        source["params"]["fill_paint"] = json!(fill);
        let mut nodes = vec![source];
        nodes.extend(straight.then(unpremultiply));
        nodes.push(blur(5));
        Case::new(
            format!("{kind} edge keeps rgb equal to alpha"),
            chain((64, 64), nodes),
            custom(|frames| {
                assert!(alpha(&frames[0], 32, 32) > 100.0, "the blur is blank");
                for pixel in frames[0].rgba.as_chunks::<4>().0 {
                    let [r, g, b, a] = *pixel;
                    assert!(
                        r.abs_diff(a) <= 2 && g.abs_diff(a) <= 2 && b.abs_diff(a) <= 2,
                        "{pixel:?}: rgb should equal alpha"
                    );
                }
            }),
        )
    };
    let fractional = swept(
        "fractional radii grow the edge alpha strictly".into(),
        (64, 64),
        vec![white(24.0, 24.0, 16, 16), blur(by_frame(&[2.0, 2.5, 3.0]))],
        3,
        custom(|frames| {
            let [a, b, c] = [0, 1, 2].map(|frame| alpha(&frames[frame], 22, 32));
            assert!(
                a + 2.0 < b && b + 2.0 < c,
                "alpha at radius 2, 2.5, 3: {a}, {b}, {c}"
            );
        }),
    );
    check([
        impulse(1.0),
        impulse(2.0),
        fractional,
        wide(blur(80), "blur of radius 80", true),
        wide(shadow([0.0, 0.0], 80), "shadow of radius 80", false),
        fringe("opaque", [255; 4], false),
        fringe("straight", [255; 4], true),
        fringe("translucent straight", [255, 255, 255, 128], true),
    ]);
}

#[test]
fn blur_and_shadow_share_a_kernel_but_not_an_edge_mode() {
    let gpu = gpu();
    let alphas = |nodes: Vec<Value>, canvas| {
        let composition = chain((canvas, canvas), nodes).into();
        render(
            &mut gpu.renderer(),
            &composition,
            0.0,
            &TestMedia::default(),
        )
    };
    let square = |effect| vec![white(24.0, 24.0, 16, 16), effect];
    let (blurred, shadowed) = (
        alphas(square(blur(6.5)), 64),
        alphas(square(shadow([0.0, 0.0], 6.5)), 64),
    );
    // Outside the square the shadow is the blurred alpha.
    let mut strongest = 0.0_f64;
    for (x, y) in [(10, 32), (20, 32), (32, 20), (43, 32), (32, 46), (4, 4)] {
        let (from_blur, from_shadow) = (alpha(&blurred, x, y), alpha(&shadowed, x, y));
        assert!(
            (from_blur - from_shadow).abs() <= 1.0,
            "({x}, {y}): blur {from_blur}, shadow {from_shadow}"
        );
        strongest = strongest.max(from_blur);
    }
    assert!(strongest > 20.0, "the blur is nearly blank: {strongest}");

    // A stripe on the canvas edge continues past it when blurred, and stops at it when shadowed.
    let stripe = |effect| vec![white(0.0, 0.0, 1, 64), effect];
    let (blurred, shadowed) = (
        alphas(stripe(blur(6)), 64),
        alphas(stripe(shadow([0.0, 0.0], 6)), 64),
    );
    let (from_blur, from_shadow) = (alpha(&blurred, 2, 32), alpha(&shadowed, 2, 32));
    assert!(
        from_shadow > 20.0 && from_blur > from_shadow + 20.0,
        "two pixels from the edge: blur {from_blur}, shadow {from_shadow}"
    );
}

#[test]
fn gaussian_continuity() {
    let mut cases = Vec::new();
    // The reduced levels fade in over radius 30.7-37.6 and 124-152. A stripe on the canvas edge
    // is extended by the clamped edge, and must keep its alpha across each switch.
    for (width, radii) in [
        (1, [34.14, 34.15]),
        (4, [138.06, 138.07]),
        (1, [28.0, 40.0]),
        (1, [123.5, 152.5]),
    ] {
        for x in [0, 512 - width] {
            cases.push(swept(
                format!("{width} px stripe at x={x}, radius {radii:?}"),
                (512, 512),
                vec![white(f64::from(x), 0.0, width, 512), blur(by_frame(&radii))],
                2,
                continuous(vec![(x, 256)], 60.0, 6.0, vec![[0, 1]]),
            ));
        }
    }
    for radii in [[34.0, 34.3], [137.5, 138.5]] {
        cases.push(swept(
            format!("a square's blur does not pop from radius {radii:?}"),
            (512, 512),
            vec![white(192.0, 192.0, 128, 128), blur(by_frame(&radii))],
            2,
            no_pop(3),
        ));
    }
    // A rectangle shifted fully off the canvas still bleeds back in at every level.
    let radii = [
        10.0, 80.0, 200.0, 34.14, 34.15, 138.06, 138.07, 28.0, 40.0, 120.0, 155.0,
    ];
    for (rect, offset, probe) in [
        (white(100.0, 200.0, 128, 128), [-230.0, 0.0], (0, 264)),
        (white(200.0, 100.0, 128, 128), [0.0, -230.0], (264, 0)),
    ] {
        cases.push(swept(
            format!("shadow shifted {offset:?} off the canvas"),
            (512, 512),
            vec![rect, shadow(offset, by_frame(&radii))],
            radii.len(),
            continuous(vec![probe], 10.0, 6.0, vec![[3, 4], [5, 6]]),
        ));
    }
    check(cases);
}

fn edge_blur_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    // Radii that blur at full resolution, on a reduced level and across two, with the shape in
    // each corner and straight alpha, so that the blur reads its edge and the canvas'. The
    // largest reaches past the canvas on every side.
    for radius in [0.0, 3.0, 8.5, 21.0, 30.0, 38.0, 40.0, 150.0] {
        for position in [[2, 3], [368, 274], [190, 140], [-5, 270]] {
            for straight in [false, true] {
                let mut nodes = vec![small(json!(position))];
                nodes.extend(straight.then(unpremultiply));
                nodes.push(blur(radius));
                let case = Case::new(
                    format!("blur {radius} at {position:?}, straight {straight}"),
                    chain((384, 288), nodes),
                    Expect::Nonblank,
                );
                cases.push(if radius < 150.0 {
                    case.bounded(true)
                } else {
                    case
                });
            }
        }
    }
    cases
}

fn animated_cases() -> Vec<Case> {
    let motion = |name: &str, nodes: Vec<Value>, canvas, duration: u32, frames: &[f64]| {
        Case::new(
            name,
            chain(canvas, nodes).frames(duration),
            Expect::Nonblank,
        )
        .frames(frames.iter().copied())
        .bounded(true)
    };
    vec![
        // The radius passes through the band where the full level hands over to the reduced one.
        motion(
            "blur radius through the quarter band",
            vec![small(json!([100, 70])), blur("=2 + frame * 9")],
            (384, 288),
            8,
            &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 7.0],
        ),
        motion(
            "blur radius through the sixteenth band",
            vec![
                small(json!([500, 380])),
                blur(by_frame(&[100.0, 124.0, 130.0, 152.0, 170.0])),
            ],
            (1100, 800),
            5,
            &[0.0, 1.0, 2.0, 3.0, 4.0],
        ),
        motion(
            "blur of a moving shape",
            vec![
                small(json!("=vec2(10 + frame * 30, 40 + frame * 10)")),
                blur(10),
            ],
            (384, 288),
            8,
            &[0.0, 2.0, 4.0, 7.0],
        ),
        motion(
            "animated shadow",
            vec![
                small(json!([40, 40])),
                tinted_shadow(
                    [json!("=frame * 12"), json!("=-frame * 8")],
                    "=4 + frame * 3",
                ),
            ],
            (384, 288),
            6,
            &[0.0, 1.0, 3.0, 5.0],
        ),
    ]
}

fn shadow_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    // Offsets that move the shadow off each edge, and one that moves it off the canvas.
    for (offset, radius) in [
        ([12.0, -9.0], 6),
        ([-30.0, 40.0], 25),
        ([0.4, 7.6], 0),
        ([-250.0, 0.0], 12),
        ([400.0, -300.0], 60),
    ] {
        for (position, straight) in [([30, 20], false), ([366, 270], true)] {
            let mut nodes = vec![small(json!(position))];
            nodes.extend(straight.then(unpremultiply));
            nodes.push(tinted_shadow(offset.map(Value::from), radius));
            cases.push(
                Case::new(
                    format!("shadow {offset:?} radius {radius} at {position:?}"),
                    chain((384, 288), nodes),
                    Expect::Nonblank,
                )
                .bounded(true),
            );
        }
    }
    cases
}

/// Values no bounds can be worked out for: the effect and its source fill the canvas.
fn unrepresentable_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for (name, effect) in [
        ("blur radius 1e6", blur(1e6)),
        ("blur radius 5e9", blur(5e9)),
        (
            "shadow offset 1e7",
            tinted_shadow([1e7.into(), 0.into()], 4),
        ),
        (
            "shadow offset -1e12",
            tinted_shadow([0.into(), (-1e12).into()], 4),
        ),
    ] {
        let nodes = vec![white(40.0, 40.0, 300, 200), effect];
        cases.push(Case::new(name, chain((384, 288), nodes), Expect::Nonblank).bounded(false));
    }
    cases
}

#[test]
fn bounds_match_reference() {
    let mut cases = edge_blur_cases();
    cases.extend(animated_cases());
    cases.extend(shadow_cases());
    cases.extend(unrepresentable_cases());
    check(cases);
}

/// Text, which needs a font, through a blur or shadow: on odd canvases, at fractional positions,
/// near the edges. Frame 1 is off the one-frame timeline.
#[test]
fn text_bounds_match_reference() {
    let row = |canvas: (u32, u32),
               at: (f64, f64),
               size: u32,
               radius: f64,
               offset: Option<(f64, f64)>| {
        let effect = match offset {
            Some((x, y)) => shadow_in([73, 137, 239, 213], 0.83, [x.into(), y.into()], radius),
            None => blur(radius),
        };
        let text = json!({ "type": "text", "params": {
            "content": "Blur edge", "font_family": "Inter", "font_size": size, "position": [at.0, at.1]
        } });
        Case::new(
            format!("{size}px text at {at:?} on {canvas:?}, radius {radius}, shadow {offset:?}"),
            chain(canvas, vec![text, effect]),
            Expect::Nonblank,
        )
        .media(TestMedia::default().font("Inter", "effects-inter", INTER))
        .frames([0.0, 1.0])
        .bounded(true)
    };
    check([
        row((1027, 773), (338.91, 309.2), 37, 34.15, Some((-67.5, 32.5))),
        row((1029, 775), (401.37, 288.6), 37, 22.4, Some((-31.5, 18.5))),
        row((383, 289), (171.3, 133.6), 29, 8.5, None),
        row((385, 291), (-3.4, 277.5), 41, 21.0, None),
        row((769, 577), (311.2, 201.1), 33, 40.0, None),
        row((769, 577), (311.2, 201.1), 33, 12.7, Some((-17.5, 26.4))),
        row((385, 291), (301.7, 22.3), 41, 30.6, Some((24.5, -9.5))),
        row((1031, 771), (355.7, 300.1), 37, 34.15, Some((-67.5, 32.5))),
        row((1111, 777), (800.77, 252.78), 37, 18.2, Some((31.5, 32.5))),
    ]);
}

#[test]
fn transform_bounds_match_reference() {
    let mut cases = Vec::new();
    // (rotate in degrees, scale, pivot, bounded, drawn). The bound is dropped past 1000 radians
    // of rotation, where the shader's sine and cosine are not exact, and past 1e6 in any value it
    // computes, where single precision cannot place the shape. Shapes that single precision
    // throws off the canvas, or scales to infinity, draw nothing in either render.
    for (rotate, scale, pivot, bounded, drawn) in [
        (5.7e4, 1.0, None, true, true),
        (5.8e4, 1.0, None, false, true),
        (1e9, 1.0, None, false, true),
        (0.0, 1e40, None, false, false),
        (0.0, 1e300, None, false, false),
        (0.0, 1.0, Some(9e5), true, true),
        (0.0, 1.0, Some(1.1e6), false, true),
        (0.0, 1.0, Some(1e7), false, true),
        (0.0, 1.0, Some(1e10), false, false),
        (30.0, 1.0, Some(1e6), true, false),
    ] {
        for animated in [false, true] {
            let param = |value: f64| {
                if animated {
                    json!(format!("={value:.0} + 0 * frame"))
                } else {
                    json!(value)
                }
            };
            let mut params = json!({
                "rotate": param(rotate), "scale_x": param(scale), "scale_y": param(scale)
            });
            if let Some(pivot) = pivot {
                params["custom_pivot"] = json!(true);
                params["pivot_x"] = param(pivot);
                params["pivot_y"] = param(pivot);
            }
            let nodes = vec![
                small(json!([40, 45])),
                json!({ "type": "transform", "params": params }),
            ];
            let expect = if drawn {
                Expect::Nonblank
            } else {
                custom(|_| {})
            };
            cases.push(
                Case::new(
                    format!(
                        "rotate {rotate}, scale {scale:e}, pivot {pivot:?}, animated {animated}"
                    ),
                    chain((256, 192), nodes).frames(2),
                    expect,
                )
                .bounded(bounded),
            );
        }
    }
    check(cases);
}
