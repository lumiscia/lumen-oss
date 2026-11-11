//! `transform` and `resize` filtering a source they show far smaller than its size.

use serde_json::{Value, json};

use lumen_engine::composition::Composition;

use crate::support::{Frame, TestMedia, gpu, render, render_working_space, scene};

const CANVAS: u32 = 256;
/// The code of the grey that is half the light of white once encoded.
const MID_GREY: u8 = 188;

/// `node` shown on the canvas with an image named `plate` for its source.
fn composition(mut node: Value) -> Composition {
    node["id"] = json!(2);
    node["in"] = json!({ "source": 1 });
    scene(
        (CANVAS, CANVAS),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
            node,
        ]),
    )
    .into()
}

fn draw(node: Value, size: u32, pixels: Vec<u8>) -> Frame {
    let media = TestMedia::default().image("plate", size, size, pixels);
    render(&mut gpu().renderer(), &composition(node), 0.0, &media)
}

fn transform(params: Value) -> Value {
    json!({ "type": "transform", "params": params })
}

fn resize(width: u32, height: u32) -> Value {
    let (x, y) = ((CANVAS - width) / 2, (CANVAS - height) / 2);
    json!({ "type": "resize", "params": { "x": x, "y": y, "width": width, "height": height } })
}

fn texels(size: u32, texel: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    (0..size * size)
        .flat_map(|index| {
            let value = texel(index % size, index / size);
            [value, value, value, 255]
        })
        .collect()
}

/// Single-texel black and white squares.
fn checker(size: u32) -> Vec<u8> {
    texels(size, |x, y| if (x + y) % 2 == 0 { 255 } else { 0 })
}

/// The pixels of the `region` square at the frame's centre, at least `margin` in from its edge.
fn inside(frame: &Frame, region: u32, margin: u32) -> Vec<[u8; 4]> {
    let low = (CANVAS - region) / 2 + margin;
    let high = (CANVAS + region) / 2 - margin;
    (low..high)
        .flat_map(|y| (low..high).map(move |x| (x, y)))
        .map(|(x, y)| frame.pixel(x, y))
        .collect()
}

fn assert_grey(pixels: &[[u8; 4]], tolerance: u8, what: &str) {
    for pixel in pixels {
        assert!(
            pixel[..3].iter().all(|c| c.abs_diff(MID_GREY) <= tolerance) && pixel[3] == 255,
            "{what}: {pixel:?} is not the grey {MID_GREY} within {tolerance}"
        );
    }
}

/// A `transform` of the canvas-sized source to `scale`, moved by `shift` of an output pixel so
/// that a single sample of each pixel falls on a texel's centre instead of between four.
fn shrunk(scale: f64, shift: f64) -> Value {
    transform(json!({
        "scale_x": scale, "scale_y": scale, "translate_x": shift, "translate_y": shift
    }))
}

#[test]
fn a_transform_averages_what_it_scales_to_a_quarter() {
    let frame = draw(shrunk(0.25, 0.125), CANVAS, checker(CANVAS));
    assert_grey(&inside(&frame, 64, 2), 2, "checker");
    // Stripes one texel wide, across and down.
    for stripes in [
        texels(CANVAS, |x, _| if x % 2 == 0 { 255 } else { 0 }),
        texels(CANVAS, |_, y| if y % 2 == 0 { 255 } else { 0 }),
    ] {
        let frame = draw(shrunk(0.25, 0.125), CANVAS, stripes);
        assert_grey(&inside(&frame, 64, 2), 2, "stripes");
    }
}

#[test]
fn a_resize_averages_what_it_shrinks_to_a_quarter() {
    // A source a little over four times the output's size, so that a single sample of each pixel
    // falls on a different part of the pattern than the last.
    let size = 258;
    let frame = draw(resize(64, 64), size, checker(size));
    assert_grey(&inside(&frame, 64, 2), 4, "checker");
    // Each mode shows the source at about a quarter of its size.
    for mode in ["stretch", "fit", "fill"] {
        let mut node = resize(64, 64);
        node["params"]["mode"] = json!(mode);
        let frame = draw(node, size, checker(size));
        assert_grey(&inside(&frame, 64, 2), 4, mode);
    }
}

#[test]
fn a_footprint_past_the_filters_span_reads_a_smaller_level() {
    // Stripes of 16 texels, which a pixel covering 32 of them averages to the mid grey however it
    // is placed, where reading only the first level would span 4 and see one colour.
    let size = 512;
    let stripes = || texels(size, |x, _| if (x / 16) % 2 == 0 { 255 } else { 0 });
    let frame = draw(
        transform(json!({
            "scale_x": 0.03125, "scale_y": 0.03125, "translate_x": -127.9, "translate_y": -127.9
        })),
        size,
        stripes(),
    );
    assert_grey(&inside(&frame, 16, 1), 3, "transform");
    let frame = draw(resize(16, 16), size, stripes());
    assert_grey(&inside(&frame, 16, 1), 3, "resize");
    // A cropped source's image is part of its texture, and shrinks the same.
    let composition: Composition = scene(
        (CANVAS, CANVAS),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
            { "id": 2, "type": "crop", "in": { "source": 1 },
              "params": { "x": 72, "y": 72, "width": 256, "height": 256 } },
            { "id": 3, "type": "resize", "in": { "source": 2 },
              "params": { "x": 124, "y": 124, "width": 8, "height": 8 } },
        ]),
    )
    .into();
    let media = TestMedia::default().image("plate", size, size, stripes());
    let frame = render(&mut gpu().renderer(), &composition, 0.0, &media);
    assert_grey(&inside(&frame, 8, 1), 3, "cropped resize");
}

#[test]
fn shrinking_one_axis_keeps_the_detail_of_the_other() {
    // Even rows are stripes a texel wide, which average to the mid grey; odd rows are black.
    let rows = texels(
        CANVAS,
        |x, y| {
            if y % 2 == 0 && x % 2 == 0 { 255 } else { 0 }
        },
    );
    // A quarter and an eighth of the width, both read from the source's first level.
    for width in [64, 32] {
        let scale = f64::from(width) / f64::from(CANVAS);
        let transformed = draw(
            transform(json!({ "scale_x": scale, "scale_y": 1.0, "translate_x": 0.125 })),
            CANVAS,
            rows.clone(),
        );
        let resized = draw(
            json!({ "type": "resize", "params": {
                "x": (CANVAS - width) / 2, "y": 0, "width": width, "height": CANVAS } }),
            CANVAS,
            rows.clone(),
        );
        for (name, frame) in [("transform", transformed), ("resize", resized)] {
            for y in 8..248 {
                for x in (CANVAS - width) / 2 + 2..(CANVAS + width) / 2 - 2 {
                    let expected = if y % 2 == 0 { MID_GREY } else { 0 };
                    let pixel = frame.pixel(x, y);
                    assert!(
                        pixel[..3].iter().all(|c| c.abs_diff(expected) <= 2),
                        "{name} to {width} wide: ({x}, {y}) is {pixel:?}, not {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_clear_border_neither_darkens_nor_fringes_what_is_shrunk() {
    // An opaque white square, with its edges between the texels a pixel covers, on clear black.
    let pixels = |x: u32, y: u32| (66..194).contains(&x) && (66..194).contains(&y);
    let plate: Vec<u8> = (0..CANVAS * CANVAS)
        .flat_map(|index| {
            if pixels(index % CANVAS, index / CANVAS) {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();
    for node in [
        transform(json!({ "scale_x": 0.25, "scale_y": 0.25 })),
        json!({ "type": "resize", "params": { "x": 0, "y": 0, "width": 64, "height": 64 } }),
    ] {
        let kind = node["type"].clone();
        let media = TestMedia::default().straight_image("plate", CANVAS, CANVAS, plate.clone());
        let frame = render(&mut gpu().renderer(), &composition(node), 0.0, &media);
        let mut partial = 0;
        for y in 0..CANVAS {
            for x in 0..CANVAS {
                let [r, g, b, a] = frame.pixel(x, y);
                if a != 0 && a != 255 {
                    partial += 1;
                }
                // The output is premultiplied by alpha as encoded, so white is as bright as it is
                // opaque wherever there is any of the square.
                assert!(
                    r.abs_diff(a) <= 2 && g.abs_diff(a) <= 2 && b.abs_diff(a) <= 2,
                    "{kind}: ({x}, {y}) is [{r}, {g}, {b}, {a}], not white at the square's edge"
                );
            }
        }
        assert!(partial > 0, "{kind}: the edge has no partial pixels");
    }
}

#[test]
fn a_rotated_shrink_is_as_even_as_an_upright_one() {
    let frame = draw(
        transform(json!({ "scale_x": 0.25, "scale_y": 0.25, "rotate": 30 })),
        CANVAS,
        checker(CANVAS),
    );
    // The square turned 30 degrees fits a 64-texel diamond whose inner square is 40 across.
    assert_grey(&inside(&frame, 40, 2), 6, "rotated");
}

/// Texels of an 8 by 8 image of varied colour and alpha, in premultiplied bytes.
fn varied() -> Vec<u8> {
    let mut seed = 0x2545_f491u32;
    let mut next = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 24) as u8
    };
    (0..64)
        .flat_map(|_| {
            let alpha = 128 + next() / 2;
            let mut texel = [0u8; 4];
            for value in &mut texel[..3] {
                *value = (u32::from(next()) * u32::from(alpha) / 255) as u8;
            }
            texel[3] = alpha;
            texel
        })
        .collect()
}

/// The working-space texels at `AT` of a 16 by 16 render of `node` over the varied image.
fn working_texels(node: Value) -> Vec<[f32; 4]> {
    let mut node = node;
    node["id"] = json!(2);
    node["in"] = json!({ "source": 1 });
    let composition: Composition = scene(
        (16, 16),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
            node,
        ]),
    )
    .into();
    let media = TestMedia::default().image("plate", 8, 8, varied());
    let texels = render_working_space(&mut gpu().renderer(), &composition, 0.0, &media);
    AT.iter().map(|&(x, y)| texels[y * 16 + x]).collect()
}

const AT: [(usize, usize); 8] = [
    (1, 1),
    (2, 5),
    (3, 3),
    (5, 2),
    (6, 6),
    (7, 4),
    (9, 9),
    (10, 3),
];

/// What the shaders before the filter wrote at `AT`, which a source shown at or above its own size
/// still gets to the bit.
const MOVED: [[f32; 4]; 8] = [
    [0.16931152, 0.5522461, 0.18164063, 0.7553711],
    [0.05331421, 0.5185547, 0.047698975, 0.7895508],
    [0.34399414, 0.13427734, 0.33520508, 0.8491211],
    [0.27563477, 0.03967285, 0.40649414, 0.7133789],
    [0.14111328, 0.29345703, 0.2849121, 0.6694336],
    [0.20593262, 0.11242676, 0.37548828, 0.79589844],
    [0.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.0],
];

const MAGNIFIED: [[f32; 4]; 8] = [
    [0.32617188, 0.1315918, 0.5644531, 0.77490234],
    [0.22875977, 0.36108398, 0.47875977, 0.81933594],
    [0.24707031, 0.21118164, 0.22436523, 0.8833008],
    [0.0036735535, 0.040222168, 0.109069824, 0.8105469],
    [0.1694336, 0.33007813, 0.097595215, 0.65234375],
    [0.09411621, 0.09411621, 0.122802734, 0.6923828],
    [0.030166626, 0.10101318, 0.2553711, 0.76464844],
    [0.21936035, 0.11315918, 0.07354736, 0.8720703],
];

const SAME_SIZE: [[f32; 4]; 8] = [
    [0.07116699, 0.7441406, 0.0064849854, 0.8388672],
    [0.016998291, 0.72998047, 0.08795166, 0.9838867],
    [0.37817383, 0.08544922, 0.046325684, 0.94091797],
    [0.5527344, 0.02368164, 0.5366211, 0.65478516],
    [0.022506714, 0.46679688, 0.38598633, 0.5019531],
    [0.028549194, 0.16015625, 0.48828125, 0.8623047],
    [0.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.0],
];

const ENLARGED: [[f32; 4]; 8] = [
    [0.0, 0.0, 0.0, 0.0],
    [0.06939697, 0.11090088, 0.064208984, 0.61328125],
    [0.18237305, 0.4819336, 0.21289063, 0.74658203],
    [0.12036133, 0.30078125, 0.06756592, 0.7314453],
    [0.31274414, 0.16967773, 0.42114258, 0.7758789],
    [0.36816406, 0.022964478, 0.13757324, 0.7714844],
    [0.36132813, 0.070739746, 0.04940796, 0.7475586],
    [0.40478516, 0.059295654, 0.55029297, 0.74902344],
];

#[test]
fn what_is_not_shrunk_is_sampled_as_it_always_was() {
    let moved = working_texels(transform(
        json!({ "translate_x": 0.37, "translate_y": -0.21 }),
    ));
    let magnified = working_texels(transform(
        json!({ "scale_x": 2.0, "scale_y": 2.5, "rotate": 17 }),
    ));
    let same_size = working_texels(
        json!({ "type": "resize", "params": { "x": 0, "y": 0, "width": 8, "height": 8 } }),
    );
    let enlarged = working_texels(
        json!({ "type": "resize", "params": { "x": 2, "y": 1, "width": 12, "height": 11 } }),
    );
    assert_eq!(moved, MOVED);
    assert_eq!(magnified, MAGNIFIED);
    assert_eq!(same_size, SAME_SIZE);
    assert_eq!(enlarged, ENLARGED);
}

#[test]
fn a_zoom_changes_every_pixel_by_little_at_each_step() {
    // The edge between white and black across the source's longer side, and a white texel alone in
    // black on a square one, held under the pivot while the scale falls from magnifying, through the
    // first footprint and the one where a chain starts (4 texels, at scale 0.25) and the levels
    // rise, to well past a level boundary, at 0.002 a frame: the most that a pixel by it can change
    // by as the filter's width and mip level move. A thin source has axes that run out of levels
    // before the other does.
    for (width, height) in [(64, 64), (3, 512), (512, 5)] {
        let (cx, cy) = (width / 2, height / 2);
        let wide = width >= height;
        let image: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                let (x, y) = (index % width, index / width);
                let on = if width == height {
                    (x, y) == (cx, cy)
                } else if wide {
                    x < cx
                } else {
                    y < cy
                };
                let value = if on { 255 } else { 0 };
                [value, value, value, 255]
            })
            .collect();
        let pivot = if width == height {
            [f64::from(cx) + 0.5, f64::from(cy) + 0.5]
        } else if wide {
            [f64::from(cx), f64::from(cy) + 0.5]
        } else {
            [f64::from(cx) + 0.5, f64::from(cy)]
        };
        let frames = 500;
        let composition: Composition = scene(
            (width, height),
            json!([
                { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
                { "id": 2, "type": "transform", "in": { "source": 1 }, "params": {
                    "scale_x": "=1.1 - 0.002 * frame", "scale_y": "=1.1 - 0.002 * frame",
                    "custom_pivot": true, "pivot_x": pivot[0], "pivot_y": pivot[1] } },
            ]),
        )
        .frames(frames)
        .into();
        let media = TestMedia::default().image("plate", width, height, image);
        let mut renderer = gpu().renderer();
        let mut last: Option<Vec<f32>> = None;
        let mut worst = 0.0_f32;
        for frame in 0..frames {
            let texels =
                render_working_space(&mut renderer, &composition, f64::from(frame), &media);
            let around: Vec<f32> = (cy.saturating_sub(2)..(cy + 3).min(height))
                .flat_map(|y| (cx.saturating_sub(2)..(cx + 3).min(width)).map(move |x| (x, y)))
                .map(|(x, y)| texels[(y * width + x) as usize][0])
                .collect();
            if let Some(last) = &last {
                for (before, after) in last.iter().zip(&around) {
                    worst = worst.max((before - after).abs());
                }
            }
            last = Some(around);
        }
        assert!(
            worst < 0.02,
            "{width}x{height}: a pixel changed by {worst} in one step"
        );
    }
}

fn solid(size: u32, color: [u8; 3]) -> Vec<u8> {
    (0..size * size)
        .flat_map(|_| [color[0], color[1], color[2], 255])
        .collect()
}

#[test]
fn a_chain_kept_for_one_source_is_not_read_for_another_that_changes() {
    // A red image through a same-size resize is kept after the first frame, and a transform
    // shrinks it as it changes. A blue image, brightened more each frame, is a source that changes,
    // and another transform shrinks it. Each thumbnail must be its own source's colour on every frame.
    let size = 128;
    // The scale changes a little each frame so that neither transform is kept.
    let shrink = |id: u32, source: u32, shift: i32| {
        json!({ "id": id, "type": "transform", "in": { "source": source },
                "params": { "scale_x": "=0.1 + 0.001 * frame", "scale_y": "=0.1 + 0.001 * frame",
                            "translate_x": shift } })
    };
    let composition: Composition = scene(
        (size, size),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "red" } },
            { "id": 2, "type": "resize", "in": { "source": 1 },
              "params": { "width": size, "height": size } },
            shrink(3, 2, -32),
            { "id": 4, "type": "media_in", "params": { "kind": "image", "source": "blue" } },
            { "id": 5, "type": "exposure", "in": { "source": 4 },
              "params": { "exposure": "=0.2 * frame" } },
            shrink(6, 5, 32),
            { "id": 7, "type": "merge", "in": { "base": 3, "overlay": 6 } },
        ]),
    )
    .frames(3)
    .into();
    let media = TestMedia::default()
        .image("red", size, size, solid(size, [255, 0, 0]))
        .image("blue", size, size, solid(size, [0, 0, 255]));
    let mut renderer = gpu().renderer();
    for frame in 0..3 {
        let shown = render(&mut renderer, &composition, f64::from(frame), &media);
        let (red, blue) = (shown.pixel(32, 64), shown.pixel(96, 64));
        assert!(
            red[0] > 200 && red[2] == 0,
            "frame {frame}: {red:?} is not red"
        );
        assert!(
            blue[2] > 200 && blue[0] == 0,
            "frame {frame}: {blue:?} is not blue"
        );
    }
}
