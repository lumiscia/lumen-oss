//! `lens_blur` on a point, whose bokeh is a flat disc or polygon of the radius, and on two halves
//! at different depths, one in focus.

use lumen_engine::gpu::GpuCompositionRenderer;
use serde_json::{Value, json};

use crate::color::linear;
use crate::support::{Frame, TestMedia, gpu, render, scene};

const CANVAS: u32 = 64;

/// A white canvas pixel at (32, 32) over black, 256 times brighter than white so its bokeh shows
/// in 8-bit output, then through `lens_blur` with `params` when there are any.
fn point(renderer: &mut GpuCompositionRenderer, lens_blur: Option<Value>) -> Frame {
    point_at(renderer, (CANVAS, CANVAS), [32, 32], lens_blur)
}

/// Like `point`, on a canvas of `size` with the pixel at `at`.
fn point_at(
    renderer: &mut GpuCompositionRenderer,
    size: (u32, u32),
    at: [u32; 2],
    lens_blur: Option<Value>,
) -> Frame {
    let mut nodes = point_nodes(at);
    if let Some(params) = lens_blur {
        nodes
            .push(json!({ "id": 5, "type": "lens_blur", "params": params, "in": { "source": 4 } }));
    }
    let composition = scene(size, Value::Array(nodes)).into();
    render(renderer, &composition, 0.0, &TestMedia::default())
}

/// Nodes 1 to 4: a white pixel at `at` over black, 256 times brighter than white.
fn point_nodes(at: [u32; 2]) -> Vec<Value> {
    vec![
        json!({ "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255] } }),
        json!({ "id": 2, "type": "shape", "params": {
            "position": at, "width": 1, "height": 1, "fill_paint": [255, 255, 255, 255] } }),
        json!({ "id": 3, "type": "raster_multimerge", "in": { "layers": [1, 2] } }),
        json!({ "id": 4, "type": "exposure", "params": { "exposure": 8 }, "in": { "source": 3 } }),
    ]
}

/// The bokeh of a point in a frame: the light of its pixels, where it is centred, and how bright
/// its middle is.
struct Bokeh<'a> {
    frame: &'a Frame,
    centre: [f64; 2],
    level: f64,
}

impl<'a> Bokeh<'a> {
    fn of(frame: &'a Frame) -> Self {
        let (mut total, mut x, mut y) = (0.0, 0.0, 0.0);
        for py in 0..frame.height {
            for px in 0..frame.width {
                let light = linear(frame.pixel(px, py)[0]);
                total += light;
                x += light * (f64::from(px) + 0.5);
                y += light * (f64::from(py) + 0.5);
            }
        }
        let centre = [x / total, y / total];
        let mut bokeh = Self {
            frame,
            centre,
            level: 0.0,
        };
        bokeh.level = bokeh.light(centre);
        bokeh
    }

    /// The light at `at`, in device pixels, between pixel centres.
    fn light(&self, at: [f64; 2]) -> f64 {
        let [x, y] = at.map(|value| value - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let texel =
            |dx: f64, dy: f64| linear(self.frame.pixel((x0 + dx) as u32, (y0 + dy) as u32)[0]);
        let top = texel(0.0, 0.0) * (1.0 - fx) + texel(1.0, 0.0) * fx;
        let bottom = texel(0.0, 1.0) * (1.0 - fx) + texel(1.0, 1.0) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    /// How far from the centre, at `degrees` clockwise from straight right, the light falls below
    /// half the middle's.
    fn edge(&self, degrees: f64) -> f64 {
        let (dy, dx) = degrees.to_radians().sin_cos();
        let mut distance = 0.0;
        while self.light([
            self.centre[0] + dx * distance,
            self.centre[1] + dy * distance,
        ]) >= self.level * 0.5
        {
            distance += 0.02;
        }
        distance
    }
}

#[test]
fn a_point_spreads_into_an_evenly_lit_disc_or_polygon_of_the_radius() {
    let gpu = gpu();
    let frame = point(&mut gpu.renderer(), Some(json!({ "radius": 10 })));
    let disc = Bokeh::of(&frame);
    // A canvas pixel's 256 spread over the disc's 314.
    let expected = 256.0 / (std::f64::consts::PI * 100.0);
    assert!(
        (disc.level / expected - 1.0).abs() < 0.08,
        "the middle is {}, not {expected}",
        disc.level
    );
    for degrees in [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
        let edge = disc.edge(degrees);
        assert!(
            (edge - 10.0).abs() < 0.75,
            "the edge at {degrees}° is {edge}"
        );
        // Flat to the edge's last two pixels.
        for distance in [0.0, 2.0, 4.0, 6.0, 8.0] {
            let (dy, dx) = f64::to_radians(degrees).sin_cos();
            let light = disc.light([
                disc.centre[0] + dx * distance,
                disc.centre[1] + dy * distance,
            ]);
            assert!(
                (light / disc.level - 1.0).abs() < 0.06,
                "{distance} out at {degrees}° is {light} against {}",
                disc.level
            );
        }
    }

    let frame = point(
        &mut gpu.renderer(),
        Some(json!({ "radius": 10, "blades": 6 })),
    );
    let hexagon = Bokeh::of(&frame);
    // A corner points straight up, and the middle of a side straight right.
    let corner = hexagon.edge(270.0);
    let side = hexagon.edge(0.0);
    assert!((corner - 10.0).abs() < 0.75, "the corner is {corner} out");
    let ratio = corner / side;
    let expected = 1.0 / 30f64.to_radians().cos();
    assert!(
        (ratio - expected).abs() < 0.05,
        "corner over side is {ratio}, not {expected}"
    );

    let frame = point(
        &mut gpu.renderer(),
        Some(json!({ "radius": 10, "blades": 3 })),
    );
    let triangle = Bokeh::of(&frame);
    // A corner points straight up, and the middle of a side straight down, half as far out.
    let (up, down) = (triangle.edge(270.0), triangle.edge(90.0));
    assert!(
        (up - 10.0).abs() < 1.0 && (down - 5.0).abs() < 1.0,
        "the triangle reaches {up} up and {down} down"
    );
}

#[test]
fn a_highlight_keeps_its_place_in_a_dark_block() {
    // At radius 200 the point is gathered in 64-pixel blocks, and it is at the left edge of one.
    let frame = point_at(
        &mut gpu().renderer(),
        (512, 64),
        [256, 32],
        Some(json!({ "radius": 200 })),
    );
    let centre = Bokeh::of(&frame).centre;
    assert!(
        (centre[0] - 256.5).abs() < 2.0,
        "the bokeh is centred at {centre:?}"
    );
}

#[test]
fn no_radius_passes_the_source_through() {
    let gpu = gpu();
    let untouched = point(&mut gpu.renderer(), None);
    let passed = point(&mut gpu.renderer(), Some(json!({ "radius": 0 })));
    assert_eq!(passed.rgba, untouched.rgba);
}

#[test]
fn bokeh_keeps_its_size_in_canvas_pixels_at_any_render_scale() {
    let gpu = gpu();
    let full = point(&mut gpu.renderer(), Some(json!({ "radius": 10 })));
    let mut renderer = gpu.renderer();
    renderer.set_render_scale(2.0).expect("a valid scale");
    let double = point(&mut renderer, Some(json!({ "radius": 10 })));
    assert_eq!((double.width, double.height), (2 * CANVAS, 2 * CANVAS));
    let (full, double) = (Bokeh::of(&full), Bokeh::of(&double));
    assert!(
        (double.level / full.level - 1.0).abs() < 0.08,
        "the middle is {} at scale 2 and {} at 1",
        double.level,
        full.level
    );
    for degrees in [0.0, 90.0, 180.0, 270.0] {
        let (once, twice) = (full.edge(degrees), double.edge(degrees) / 2.0);
        assert!(
            (once - twice).abs() < 0.5,
            "the edge at {degrees}° is {once} at scale 1 and {twice} at 2"
        );
    }
}

#[test]
fn a_large_radius_costs_a_few_passes_more_than_a_small_one() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let mut passes = |params: Option<Value>| {
        let frame = point(&mut renderer, params);
        (frame, renderer.recorded_pass_count())
    };
    let (_, none) = passes(None);
    let (_, small) = passes(Some(json!({ "radius": 4 })));
    let (frame, large) = passes(Some(json!({ "radius": 200 })));
    // A preparation, a reduction per level and a resolve: levels to half of 200 / 6 and one more.
    assert_eq!(small - none, 3);
    assert_eq!(large - none, 9);
    // The point still lights the whole canvas, a disc 200 pixels across.
    let corner = linear(frame.pixel(0, 0)[0]);
    let expected = 256.0 / (std::f64::consts::PI * 200.0 * 200.0);
    assert!(
        (corner / expected - 1.0).abs() < 0.25,
        "the corner is {corner}, not {expected}"
    );
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// Where the near half ends: off the levels' block boundaries, so the blocks across it mix near
/// and far.
const SPLIT: u32 = 33;

/// Red and black stripes, two pixels wide, left of `SPLIT` at depth 0, and green and blue ones
/// right of it at depth 1, through `lens_blur` with `params` when there are any.
fn halves(lens_blur: Option<Value>) -> Frame {
    let stripes = |x: u32| match (x < SPLIT, (x / 2).is_multiple_of(2)) {
        (true, true) => RED,
        (true, false) => BLACK,
        (false, true) => GREEN,
        (false, false) => BLUE,
    };
    let depth = |x: u32| if x < SPLIT { BLACK } else { [255; 4] };
    columns((64, 16), &stripes, 64, &depth, lens_blur)
}

/// A source of `size` coloured by column with `color`, and a depth as tall and `depth_width` wide
/// coloured with `depth`, through `lens_blur` with `params` when there are any.
fn columns(
    (width, height): (u32, u32),
    color: &dyn Fn(u32) -> [u8; 4],
    depth_width: u32,
    depth: &dyn Fn(u32) -> [u8; 4],
    lens_blur: Option<Value>,
) -> Frame {
    let pixels = |width: u32, color: &dyn Fn(u32) -> [u8; 4]| -> Vec<u8> {
        (0..width * height)
            .flat_map(|index| color(index % width))
            .collect()
    };
    let media = TestMedia::default()
        .image("stripes", width, height, pixels(width, color))
        .image("depth", depth_width, height, pixels(depth_width, depth));
    let mut nodes = vec![
        json!({ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "depth" } }),
        json!({ "id": 2, "type": "media_in", "params": { "kind": "image", "source": "stripes" } }),
    ];
    if let Some(params) = lens_blur {
        nodes.push(json!({ "id": 3, "type": "lens_blur", "params": params,
            "in": { "source": 2, "depth": 1 } }));
    }
    let composition = scene((width, height), Value::Array(nodes)).into();
    render(&mut gpu().renderer(), &composition, 0.0, &media)
}

#[test]
fn a_blurred_background_stays_behind_a_sharp_foreground() {
    let source = halves(None);
    let frame = halves(Some(
        json!({ "radius": 8, "focus": 0, "focus_range": 0.05 }),
    ));
    for y in 0..16 {
        for x in 0..SPLIT {
            assert_eq!(
                frame.pixel(x, y),
                source.pixel(x, y),
                "({x}, {y}) is in focus"
            );
        }
        for x in SPLIT..64 {
            let [red, green, blue, _] = frame.pixel(x, y);
            assert_eq!(red, 0, "({x}, {y}) shows the foreground");
            // The stripes blur into an even mix, away from the canvas's edge, which repeats.
            if (SPLIT + 8..56).contains(&x) {
                assert!(
                    (150..=225).contains(&green) && (150..=225).contains(&blue),
                    "({x}, {y}) is {green} green and {blue} blue"
                );
            }
        }
    }
}

#[test]
fn a_blurred_foreground_spreads_over_a_sharp_background() {
    let source = halves(None);
    let frame = halves(Some(
        json!({ "radius": 8, "focus": 1, "focus_range": 0.05 }),
    ));
    for y in 0..16 {
        // Red spreads over the background nearly as far as the radius.
        for x in SPLIT..SPLIT + 4 {
            assert!(
                frame.pixel(x, y)[0] > 60,
                "({x}, {y}) is {:?}",
                frame.pixel(x, y)
            );
        }
        for x in SPLIT + 10..64 {
            assert_eq!(
                frame.pixel(x, y),
                source.pixel(x, y),
                "({x}, {y}) is in focus"
            );
        }
        // The foreground's stripes blur into an even mix.
        for x in 4..SPLIT - 10 {
            let red = frame.pixel(x, y)[0];
            assert!((150..=225).contains(&red), "({x}, {y}) is {red} red");
        }
    }
}

#[test]
fn a_slightly_blurred_foreground_spreads_however_wide_the_radius() {
    // With the far half at the focus, the near half's depth is about 0.44, a CoC of about 8 of
    // the radius's 200.
    let params = json!({ "radius": 200, "focus": 0.5, "focus_range": 0.05, "aperture": 4 });
    let stripes = |x: u32| match (x < SPLIT, (x / 2).is_multiple_of(2)) {
        (true, true) => RED,
        (true, false) => BLACK,
        (false, _) => GREEN,
    };
    let depth = |x: u32| if x < SPLIT { [112; 4] } else { [128; 4] };
    let source = columns((64, 16), &stripes, 64, &depth, None);
    let frame = columns((64, 16), &stripes, 64, &depth, Some(params));
    for y in 0..16 {
        for x in SPLIT..SPLIT + 4 {
            assert!(
                frame.pixel(x, y)[0] > 60,
                "({x}, {y}) is {:?}",
                frame.pixel(x, y)
            );
        }
        for x in SPLIT + 12..64 {
            assert_eq!(
                frame.pixel(x, y),
                source.pixel(x, y),
                "({x}, {y}) is in focus"
            );
        }
    }
}

#[test]
fn a_thin_blurred_foreground_spreads_from_blocks_it_shares() {
    // An 8-pixel red stripe in front, blurred by 64, over a sharp green background: gathered in
    // 16-pixel blocks, each holding some of both.
    let stripe = |x: u32| (100..108).contains(&x);
    let color = |x: u32| if stripe(x) { RED } else { GREEN };
    let depth = |x: u32| if stripe(x) { BLACK } else { [255; 4] };
    let params = json!({ "radius": 64, "focus": 1, "focus_range": 0.05 });
    let source = columns((256, 16), &color, 256, &depth, None);
    let frame = columns((256, 16), &color, 256, &depth, Some(params));
    for y in 0..16 {
        // The stripe covers about 8 of every 100 or so pixels of a disc this far out.
        for x in (108..150).chain(50..100) {
            let red = frame.pixel(x, y)[0];
            assert!(red > 40, "({x}, {y}) is {red} red");
        }
        for x in (0..20).chain(190..256) {
            assert_eq!(
                frame.pixel(x, y),
                source.pixel(x, y),
                "({x}, {y}) is out of reach"
            );
        }
    }
}

#[test]
fn a_depth_smaller_than_the_canvas_is_missing_past_its_edge() {
    let stripes = |x: u32| {
        if (x / 2).is_multiple_of(2) {
            GREEN
        } else {
            BLUE
        }
    };
    let white = |_| [255; 4];
    let params = json!({ "radius": 8, "focus": 1, "focus_range": 0.05 });
    let source = columns((128, 16), &stripes, 64, &white, None);
    let frame = columns((128, 16), &stripes, 64, &white, Some(params));
    for y in 0..16 {
        // In focus where the depth is white.
        for x in 0..50 {
            assert_eq!(
                frame.pixel(x, y),
                source.pixel(x, y),
                "({x}, {y}) is in focus"
            );
        }
        // Past it, depth 0 is far from the focus, so the stripes blur into an even mix.
        for x in 72..120 {
            let [_, green, blue, _] = frame.pixel(x, y);
            assert!(
                (150..=225).contains(&green) && (150..=225).contains(&blue),
                "({x}, {y}) is {green} green and {blue} blue"
            );
        }
    }
}

#[test]
fn a_source_sized_to_its_bounds_blurs_as_a_full_frame_one_does() {
    let gpu = gpu();
    let composition = scene(
        (CANVAS, CANVAS),
        json!([
            { "id": 1, "type": "shape", "params": {
                "position": [26, 28], "width": 6, "height": 4, "fill_paint": [255, 255, 255, 255] } },
            { "id": 2, "type": "exposure", "params": { "exposure": 6 }, "in": { "source": 1 } },
            { "id": 3, "type": "lens_blur", "params": { "radius": 20, "blades": 5 }, "in": { "source": 2 } },
        ]),
    )
    .into();
    let bounded = render(
        &mut gpu.renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    );
    let mut reference = gpu.renderer();
    reference.set_full_frame_sources(true);
    let full = render(&mut reference, &composition, 0.0, &TestMedia::default());
    let differs = bounded
        .rgba
        .iter()
        .zip(&full.rgba)
        .filter(|(a, b)| a.abs_diff(**b) > 1)
        .count();
    assert_eq!(differs, 0, "{differs} channels differ");
    // The bokeh reaches 20 pixels past the shape, beyond the shape's own bounds.
    assert!(full.pixel(26 - 15, 30)[3] > 0);
}

/// A point at (32, 32) through `lens_blur` with `params`, masked by a solid `paint` over the
/// whole canvas.
fn masked_point(paint: [u8; 4], params: Value) -> Frame {
    let mut nodes = point_nodes([32, 32]);
    nodes.extend([
        json!({ "id": 6, "type": "background", "params": { "paint": paint } }),
        json!({ "id": 5, "type": "lens_blur", "params": params, "in": { "source": 4, "mask": 6 } }),
    ]);
    let composition = scene((CANVAS, CANVAS), Value::Array(nodes)).into();
    render(
        &mut gpu().renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    )
}

#[test]
fn a_masked_point_stays_a_point_while_an_unmasked_one_becomes_a_disc() {
    // Two points; a square mask around the left one.
    let nodes = json!([
        { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255] } },
        { "id": 2, "type": "shape", "params": {
            "position": [16, 32], "width": 1, "height": 1, "fill_paint": [255, 255, 255, 255] } },
        { "id": 3, "type": "shape", "params": {
            "position": [48, 32], "width": 1, "height": 1, "fill_paint": [255, 255, 255, 255] } },
        { "id": 4, "type": "raster_multimerge", "in": { "layers": [1, 2, 3] } },
        { "id": 5, "type": "exposure", "params": { "exposure": 8 }, "in": { "source": 4 } },
        { "id": 6, "type": "shape", "params": {
            "position": [8, 24], "width": 16, "height": 16, "fill_paint": [255, 255, 255, 255] } },
        { "id": 7, "type": "lens_blur", "params": { "radius": 8 }, "in": { "source": 5, "mask": 6 } },
    ]);
    let composition = scene((CANVAS, CANVAS), nodes).into();
    let frame = render(
        &mut gpu().renderer(),
        &composition,
        0.0,
        &TestMedia::default(),
    );
    assert_eq!(frame.pixel(16, 32), [255; 4], "the masked point stays");
    for (x, y) in [(15, 32), (17, 32), (16, 31), (16, 33), (19, 32), (13, 32)] {
        assert_eq!(frame.pixel(x, y), [0, 0, 0, 255], "({x}, {y}) beside it");
    }
    for (x, y) in [(48, 32), (53, 32), (43, 32), (48, 37), (48, 27)] {
        assert!(frame.pixel(x, y)[0] > 200, "({x}, {y}) is in the disc");
    }
    assert_eq!(frame.pixel(48 + 12, 32)[0], 0, "past the disc");
}

#[test]
fn a_mask_reads_the_channel_its_mode_names() {
    // Red on the left, green on the right, all opaque.
    let stripes = |x: u32| if x < SPLIT { RED } else { GREEN };
    let colour = |x: u32| {
        if (x / 2).is_multiple_of(2) {
            BLUE
        } else {
            BLACK
        }
    };
    let render_with = |mode: &str| {
        let pixels = |color: &dyn Fn(u32) -> [u8; 4]| -> Vec<u8> {
            (0..64 * 16).flat_map(|index| color(index % 64)).collect()
        };
        let media = TestMedia::default()
            .image("colour", 64, 16, pixels(&colour))
            .image("mask", 64, 16, pixels(&stripes));
        let nodes = json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "colour" } },
            { "id": 2, "type": "media_in", "params": { "kind": "image", "source": "mask" } },
            { "id": 3, "type": "lens_blur", "params": { "radius": 8, "mask_mode": mode },
              "in": { "source": 1, "mask": 2 } },
        ]);
        render(
            &mut gpu().renderer(),
            &scene((64, 16), nodes).into(),
            0.0,
            &media,
        )
    };
    let source = columns((64, 16), &colour, 64, &|_| BLACK, None);
    // `source`'s stripes are blue and black, sharp where the mask is read as 1.
    let sharp = |frame: &Frame, from: u32, to: u32| {
        (0..16).all(|y| (from..to).all(|x| frame.pixel(x, y) == source.pixel(x, y)))
    };
    let red = render_with("red");
    assert!(sharp(&red, 0, SPLIT - 12), "red keeps the left sharp");
    assert!(!sharp(&red, SPLIT + 12, 52), "and blurs the right");
    let green = render_with("green");
    assert!(sharp(&green, SPLIT + 12, 52), "green keeps the right sharp");
    assert!(!sharp(&green, 0, SPLIT - 12), "and blurs the left");
    let alpha = render_with("alpha");
    assert!(sharp(&alpha, 12, 52), "alpha is 1 everywhere");
}

#[test]
fn a_masked_object_neither_spreads_nor_is_covered_by_the_blur_around_it() {
    // Red stripes on the left under a mask, green and blue ones on the right without.
    let color = |x: u32| match (x < SPLIT, (x / 2).is_multiple_of(2)) {
        (true, true) => RED,
        (true, false) => BLACK,
        (false, true) => GREEN,
        (false, false) => BLUE,
    };
    let mask = |x: u32| if x < SPLIT { [255; 4] } else { [0; 4] };
    let pixels = |color: &dyn Fn(u32) -> [u8; 4]| -> Vec<u8> {
        (0..64 * 16).flat_map(|index| color(index % 64)).collect()
    };
    let media = TestMedia::default()
        .image("stripes", 64, 16, pixels(&color))
        .image("mask", 64, 16, pixels(&mask));
    let render_with = |mask: bool| {
        let mut lens = json!({ "id": 3, "type": "lens_blur", "params": { "radius": 8 },
            "in": { "source": 1 } });
        if mask {
            lens["in"]["mask"] = json!(2);
        }
        let nodes = json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "stripes" } },
            { "id": 2, "type": "media_in", "params": { "kind": "image", "source": "mask" } },
            lens,
        ]);
        render(
            &mut gpu().renderer(),
            &scene((64, 16), nodes).into(),
            0.0,
            &media,
        )
    };
    let source = halves(None);
    let frame = render_with(true);
    for y in 0..16 {
        for x in 0..SPLIT {
            assert_eq!(
                frame.pixel(x, y),
                source.pixel(x, y),
                "({x}, {y}) is masked"
            );
        }
        // The block of 2 pixels across the mask's edge holds both, so the pixels beside it show a
        // little of the masked side.
        for x in SPLIT + 3..64 {
            assert_eq!(frame.pixel(x, y)[0], 0, "({x}, {y}) shows the masked red");
        }
        for x in SPLIT + 8..56 {
            let [_, green, blue, _] = frame.pixel(x, y);
            assert!(
                (150..=225).contains(&green) && (150..=225).contains(&blue),
                "({x}, {y}) is {green} green and {blue} blue"
            );
        }
    }
    // Without the mask the red does spread, so the test would tell.
    let unmasked = render_with(false);
    assert!(unmasked.pixel(SPLIT + 3, 8)[0] > 20);
}

#[test]
fn half_a_mask_halves_the_blur() {
    let radius = 16.0;
    let whole_frame = masked_point([0, 0, 0, 0], json!({ "radius": radius }));
    let unmasked = Bokeh::of(&whole_frame);
    // An alpha of 128 of 255 is a mask of about a half.
    let half_frame = masked_point([255, 255, 255, 128], json!({ "radius": radius }));
    let half = Bokeh::of(&half_frame);
    for degrees in [0.0, 90.0, 180.0, 270.0] {
        let whole = unmasked.edge(degrees);
        assert!((whole - radius).abs() < 0.75, "unmasked reaches {whole}");
        let edge = half.edge(degrees);
        assert!(
            (edge - radius / 2.0).abs() < 0.75,
            "half masked reaches {edge} at {degrees}°"
        );
    }
}
