//! Shapes, paths, transforms, sampling, crop and resize against hand-computed pixels and
//! independent geometry: supersampled outlines and exact ellipse distances.

use std::f64::consts::{PI, TAU};

use serde_json::{Value, json};

use crate::support::{Case, Expect, Frame, Scene, TestMedia, check, gpu, render, scene};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn one_node(size: (u32, u32), kind: &str, params: Value) -> Scene {
    scene(size, json!([{ "id": 1, "type": kind, "params": params }]))
}

fn shape(size: (u32, u32), params: Value) -> Scene {
    one_node(size, "shape", params)
}

fn path(size: (u32, u32), params: Value) -> Scene {
    one_node(size, "path", params)
}

/// `params` with the fields of `more` added over them.
fn with(mut params: Value, more: Value) -> Value {
    params
        .as_object_mut()
        .unwrap()
        .extend(more.as_object().unwrap().clone());
    params
}

fn pixels(frame: &Frame) -> impl Iterator<Item = &[u8; 4]> {
    frame.rgba.as_chunks::<4>().0.iter()
}

fn alpha_of(frame: &Frame, x: usize, y: usize) -> u8 {
    frame.pixel(x as u32, y as u32)[3]
}

/// The largest difference between each pixel's alpha and `expected(x, y)`, and where it is.
fn max_alpha_error(frame: &Frame, expected: impl Fn(usize, usize) -> u8) -> (u8, (usize, usize)) {
    let width = frame.width as usize;
    let (error, index) = pixels(frame)
        .enumerate()
        .map(|(index, pixel)| {
            (
                pixel[3].abs_diff(expected(index % width, index / width)),
                index,
            )
        })
        .max()
        .unwrap();
    (error, (index % width, index / width))
}

fn has_alpha_above(frame: &Frame, limit: u8) -> bool {
    pixels(frame).any(|pixel| pixel[3] > limit)
}

fn shape_coverage() -> Vec<Case> {
    let triangle = |points: &str| {
        shape(
            (16, 16),
            json!({
                "geometry_kind": "polygon", "polygon_points": points, "position": [2, 2],
                "fill_paint": RED
            }),
        )
    };
    let inside_triangle = vec![(3, 3, RED), (10, 3, RED), (3, 10, RED), (6, 6, RED)];
    let outside_triangle =
        [(13, 13), (12, 8), (8, 12), (0, 0), (15, 15)].map(|(x, y)| (x, y, CLEAR));
    let probes =
        |inside: &[(u32, u32, [u8; 4])]| Expect::Probes([inside, &outside_triangle].concat());

    // A line 4 wide that crosses the canvas, with a stroke at width 0 or below it.
    let line = |stroke_width: f64| {
        path(
            (64, 64),
            json!({
                "data": "M 10.5 10.5 L 50.5 10.5", "fill_enabled": false, "stroke_enabled": true,
                "stroke_width": stroke_width
            }),
        )
    };
    let closed = "M 10.3 10.7 L 60.2 20.1 L 30.4 70.6 Z";
    let open = "M 10.3 10.7 L 60.2 20.1 L 30.4 70.6";
    // Even-odd leaves a pentagram's points as holes in a square and fills its centre.
    let polyline = |count: usize| {
        let mut data = String::from("M 10 10");
        for index in 1..count - 2 {
            let x = 10.0 + 100.0 * index as f64 / (count - 3) as f64;
            data += &format!(" L {x:.4} {}", if index % 2 == 0 { 10.0 } else { 11.0 });
        }
        data + " L 110 90 L 10 90 Z"
    };
    let zero_size = |width: u32, height: u32| {
        shape(
            (8, 4),
            json!({
                "position": [3, 0], "width": width, "height": height, "stroke_enabled": true,
                "stroke_paint": GREEN, "fill_paint": RED
            }),
        )
    };

    vec![
        Case::new(
            "polygon draws its points, not a rectangle",
            triangle("0,0; 12,0; 0,12"),
            probes(&inside_triangle),
        ),
        Case::new(
            "polygon with invalid points draws nothing, and not the points before them",
            shape(
                (16, 16),
                json!({
                    "geometry_kind": "polygon", "position": [2, 2], "fill_paint": RED,
                    "polygon_points": "=if(frame == 1, \"\", \"0,0; 12,0; 0,12\")"
                }),
            )
            .frames(3),
            Expect::PerFrame(vec![
                probes(&inside_triangle),
                Expect::Uniform(CLEAR),
                probes(&inside_triangle),
            ]),
        )
        .frames([0.0, 1.0, 2.0]),
        Case::new(
            "rounded rectangle clears its corners",
            shape(
                (20, 20),
                json!({
                    "width": 16, "height": 16, "border_radius": 8, "position": [2, 2],
                    "fill_paint": RED
                }),
            ),
            Expect::Probes(vec![
                (10, 10, RED),
                (10, 3, RED),
                (3, 10, RED),
                (16, 10, RED),
                (10, 16, RED),
                (2, 2, CLEAR),
                (17, 2, CLEAR),
                (2, 17, CLEAR),
                (17, 17, CLEAR),
                (0, 0, CLEAR),
                (19, 19, CLEAR),
            ]),
        ),
        Case::new(
            "ellipse fills its inscribed ellipse",
            shape(
                (24, 18),
                json!({
                    "geometry_kind": "ellipse", "width": 20, "height": 10, "position": [2, 4],
                    "fill_paint": GREEN
                }),
            ),
            Expect::Probes(vec![
                (12, 9, GREEN),
                (4, 9, GREEN),
                (19, 9, GREEN),
                (12, 5, GREEN),
                (12, 12, GREEN),
                (2, 4, CLEAR),
                (21, 4, CLEAR),
                (2, 13, CLEAR),
                (21, 13, CLEAR),
                (12, 2, CLEAR),
                (12, 15, CLEAR),
            ]),
        ),
        Case::new("a shape 1 wide and 4 tall draws", zero_size(1, 4), {
            // Half the pixel is stroke, over the fill.
            Expect::Probes(vec![
                (3, 1, [128, 128, 0, 255]),
                (3, 2, [128, 128, 0, 255]),
                (0, 1, CLEAR),
                (6, 1, CLEAR),
            ])
        }),
        Case::new(
            "a shape with no width draws nothing",
            zero_size(0, 4),
            Expect::Uniform(CLEAR),
        ),
        Case::new(
            "a shape with no height draws nothing",
            zero_size(8, 0),
            Expect::Uniform(CLEAR),
        ),
        Case::new(
            "a shape with no size draws nothing",
            zero_size(0, 0),
            Expect::Uniform(CLEAR),
        ),
        Case::new(
            "path fill ignores the stroke trim range",
            path(
                (8, 4),
                json!({
                    "data": "M 1.25 0.75 L 6.75 0.75 L 6.75 3.25 L 1.25 3.25 Z",
                    "stroke_enabled": false, "trim_start": "=frame * 0.5",
                    "trim_end": "=1 - frame * 0.5"
                }),
            )
            .frames(2),
            Expect::Custom(Box::new(|frames| {
                assert_eq!(
                    frames[0].rgba, frames[1].rgba,
                    "trimming the stroke changed the fill"
                );
                // The pixel centre is 0.25 inside the left edge and 0.25 outside the top one.
                for (x, y, alpha) in [(3, 2, 255), (1, 0, 64), (0, 0, 0)] {
                    let actual = alpha_of(&frames[0], x, y);
                    assert!(actual.abs_diff(alpha) <= 1, "({x}, {y}) is {actual}");
                }
            })),
        )
        .frames([0.0, 1.0]),
        Case::new(
            "an open path fills like the same path closed",
            path((80, 80), json!({ "data": open })),
            Expect::Matches(path((80, 80), json!({ "data": closed })).into()),
        )
        .tolerance(0),
        Case::new(
            "a closed path fills its inside",
            path((80, 80), json!({ "data": closed })),
            Expect::Probes(vec![(34, 34, WHITE), (70, 60, CLEAR), (5, 70, CLEAR)]),
        ),
        Case::new("a stroke 4 wide draws", line(4.0), {
            Expect::Probes(vec![(30, 10, BLACK), (30, 14, CLEAR), (30, 6, CLEAR)])
        }),
        Case::new(
            "a stroke of no width draws nothing",
            line(0.0),
            Expect::Uniform(CLEAR),
        ),
        Case::new(
            "a stroke of negative width draws nothing",
            line(-2.0),
            Expect::Uniform(CLEAR),
        ),
        Case::new(
            "a path with more than 128 points fills inside its outline",
            path((128, 128), json!({ "data": polyline(400) })),
            Expect::Probes(vec![
                (15, 50, WHITE),
                (60, 50, WHITE),
                (105, 50, WHITE),
                (105, 85, WHITE),
                (5, 50, CLEAR),
                (115, 50, CLEAR),
                (60, 95, CLEAR),
            ]),
        ),
        banded_path_fill(),
    ]
}

/// A square holding a pentagram, filled with even-odd coverage and stroked, then scaled and
/// rotated. Every pixel clear of an edge is compared with a point-in-polygon test.
fn banded_path_fill() -> Case {
    let star: Vec<[f64; 2]> = (0..5)
        .map(|index| {
            let angle = (index * 2) as f64 * TAU / 5.0;
            [64.0 + 50.0 * angle.sin(), 64.0 - 50.0 * angle.cos()]
        })
        .collect();
    let square = [[6.0, 6.0], [122.0, 6.0], [122.0, 122.0], [6.0, 122.0]];
    let contours = [square.to_vec(), star];
    let mut data = String::new();
    let mut segments = Vec::new();
    for contour in &contours {
        for (index, &from) in contour.iter().enumerate() {
            let to = contour[(index + 1) % contour.len()];
            segments.push((from, to));
            for step in 0..300 {
                let t = step as f64 / 300.0;
                let command = if index == 0 && step == 0 { "M" } else { "L" };
                data += &format!(
                    "{command} {:.4} {:.4} ",
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t
                );
            }
        }
        data += "Z ";
    }
    let (scale, rotation) = ((1.1, 0.8), 20.0_f64);
    Case::new(
        "banded path fill keeps holes and self-intersections",
        path(
            (128, 128),
            json!({
                "data": data, "fill_paint": RED, "stroke_enabled": true, "stroke_paint": BLUE,
                "stroke_width": 3, "scale": [scale.0, scale.1], "rotation": rotation,
                "anchor": [64, 64]
            }),
        ),
        Expect::Custom(Box::new(move |frames| {
            let (sin, cos) = rotation.to_radians().sin_cos();
            // Stroked, filled, holes in the square, and outside it.
            let mut checked = [0; 4];
            for y in 0..128 {
                for x in 0..128 {
                    let (dx, dy) = (x as f64 + 0.5 - 64.0, y as f64 + 0.5 - 64.0);
                    let local = [
                        64.0 + (cos * dx + sin * dy) / scale.0,
                        64.0 + (-sin * dx + cos * dy) / scale.1,
                    ];
                    let mut inside = false;
                    let mut distance = f64::MAX;
                    for &(a, b) in &segments {
                        if (a[1] > local[1]) != (b[1] > local[1])
                            && local[0] < (b[0] - a[0]) * (local[1] - a[1]) / (b[1] - a[1]) + a[0]
                        {
                            inside = !inside;
                        }
                        let ab = [b[0] - a[0], b[1] - a[1]];
                        let h = (((local[0] - a[0]) * ab[0] + (local[1] - a[1]) * ab[1])
                            / (ab[0] * ab[0] + ab[1] * ab[1]))
                            .clamp(0.0, 1.0);
                        distance = distance
                            .min((local[0] - a[0] - ab[0] * h).hypot(local[1] - a[1] - ab[1] * h));
                    }
                    // The stroke covers 1.5 untransformed pixels either side of an edge, which is
                    // 1.25 canvas pixels at 0.8x, so a pixel farther than 4 from it is clear.
                    let expected = if distance < 0.5 {
                        BLUE
                    } else if distance > 1.5 + 2.5 {
                        if inside { RED } else { CLEAR }
                    } else {
                        continue;
                    };
                    let in_square = local.iter().all(|value| (6.0..122.0).contains(value));
                    let class = match (distance < 0.5, inside, in_square) {
                        (true, _, _) => 0,
                        (_, true, _) => 1,
                        (_, false, true) => 2,
                        (_, false, false) => 3,
                    };
                    checked[class] += 1;
                    assert_eq!(
                        frames[0].pixel(x, y),
                        expected,
                        "pixel ({x}, {y}), inside {inside}, distance {distance:.2}"
                    );
                }
            }
            assert!(checked.iter().all(|&count| count > 300), "{checked:?}");
        })),
    )
}

/// A scaled drawable against the same geometry drawn at its final size. A separate row shows
/// that the direct drawing is not blank.
fn equivalent_to_direct(
    name: &str,
    tolerance: u8,
    scaled: Scene,
    direct: impl Fn() -> Scene,
) -> Case {
    Case::new(name, scaled, Expect::Matches(direct().into())).tolerance(tolerance)
}

fn transform_equivalence() -> Vec<Case> {
    let ellipse_stroke = |params: Value| {
        with(
            json!({
                "geometry_kind": "ellipse", "position": [20.25, 30.5], "stroke_enabled": true,
                "fill_paint": RED, "stroke_paint": BLUE
            }),
            params,
        )
    };
    let direct_ellipse = || {
        shape(
            (128, 128),
            ellipse_stroke(json!({ "width": 80, "height": 72, "stroke_width": 6 })),
        )
    };
    let direct_rectangle = || {
        shape(
            (128, 128),
            json!({ "width": 80, "height": 60, "position": [25.3, 25.6] }),
        )
    };
    let edge_rectangle = || {
        shape(
            (128, 128),
            json!({ "width": 100, "height": 10, "position": [8, 8.2] }),
        )
    };
    let anisotropic = json!({ "position": [8, 8.2], "scale": [100, 0.1] });

    vec![
        equivalent_to_direct(
            "stroked ellipse scaled 8x",
            2,
            shape(
                (128, 128),
                ellipse_stroke(json!({
                    "width": 10, "height": 9, "scale": [8, 8], "stroke_width": 0.75
                })),
            ),
            direct_ellipse,
        ),
        Case::new(
            "the directly drawn stroked ellipse has fill and stroke",
            direct_ellipse(),
            Expect::Probes(vec![(60, 66, RED), (60, 30, BLUE), (2, 2, CLEAR)]),
        ),
        // Corner distances under non-uniform scale are a first-order estimate.
        equivalent_to_direct(
            "rectangle scaled 8x2 about an anchor",
            32,
            shape(
                (128, 128),
                json!({
                    "width": 10, "height": 30, "position": [60.3, 40.6], "anchor": [5, 15],
                    "scale": [8, 2]
                }),
            ),
            direct_rectangle,
        ),
        Case::new(
            "the directly drawn rectangle is filled",
            direct_rectangle(),
            Expect::Probes(vec![(65, 55, WHITE), (20, 55, CLEAR), (110, 55, CLEAR)]),
        ),
        equivalent_to_direct(
            "shape scaled (100, 0.1) antialiases the nearest canvas edge",
            2,
            shape(
                (128, 128),
                with(anisotropic.clone(), json!({ "width": 1, "height": 100 })),
            ),
            edge_rectangle,
        ),
        equivalent_to_direct(
            "path scaled (100, 0.1) antialiases the nearest canvas edge",
            2,
            path(
                (128, 128),
                with(anisotropic, json!({ "data": "M0 0 H1 V100 H0 Z" })),
            ),
            edge_rectangle,
        ),
        Case::new(
            "the directly drawn rectangle has a partly covered edge",
            edge_rectangle(),
            Expect::Probes(vec![(58, 8, [204; 4]), (30, 12, WHITE), (30, 4, CLEAR)]),
        ),
    ]
}

/// A shape's own rotation, scale and anchor against a transform node about the same pivot.
#[test]
fn shape_rotation_and_anchor_match_the_transform_node() {
    let rounded = |transformed: bool| {
        json!({
            "width": 48, "height": 24, "border_radius": 6, "position": [30, 50],
            "stroke_enabled": true, "stroke_width": 3, "fill_paint": [255, 128, 0, 255],
            "stroke_paint": [0, 64, 255, 255], "anchor": [12, 6],
            "scale": if transformed { [1.25, 0.8] } else { [1.0, 1.0] },
            "rotation": if transformed { 30.0 } else { 0.0 },
        })
    };
    let own = shape((128, 128), rounded(true));
    let via_node = scene(
        (128, 128),
        json!([
            { "id": 1, "type": "shape", "params": rounded(false) },
            { "id": 2, "type": "transform", "in": { "source": 1 }, "params": {
                "scale_x": 1.25, "scale_y": 0.8, "rotate": 30, "pivot_x": 42, "pivot_y": 56,
                "custom_pivot": true
            } },
        ]),
    );
    let gpu = gpu();
    let media = TestMedia::default();
    let own = render(&mut gpu.renderer(), &own.into(), 0.0, &media);
    let via_node = render(&mut gpu.renderer(), &via_node.into(), 0.0, &media);

    let alpha = |frame: &Frame| -> u64 { pixels(frame).map(|pixel| u64::from(pixel[3])).sum() };
    let differences: Vec<u8> = own
        .rgba
        .iter()
        .zip(&via_node.rgba)
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let mean = differences.iter().map(|&d| f64::from(d)).sum::<f64>() / differences.len() as f64;
    assert!(alpha(&via_node) > 0, "the transform node drew nothing");
    assert!(
        alpha(&own).abs_diff(alpha(&via_node)) * 100 < alpha(&via_node),
        "coverage {} against {}",
        alpha(&own),
        alpha(&via_node)
    );
    assert!(mean < 1.0, "mean difference {mean}");
    // Only the resampled edges of the raster may differ noticeably.
    assert!(*differences.iter().max().unwrap() < 128);
}

/// Maps a canvas point to untransformed pixels with the drawable transform's conventions.
fn drawable_to_local(
    point: (f64, f64),
    pivot: (f64, f64),
    scale: (f64, f64),
    rotation: f64,
) -> (f64, f64) {
    let (sin, cos) = rotation.to_radians().sin_cos();
    let (dx, dy) = (point.0 - pivot.0, point.1 - pivot.1);
    (
        pivot.0 + (cos * dx + sin * dy) / scale.0,
        pivot.1 + (cos * dy - sin * dx) / scale.1,
    )
}

/// Alpha of an outline in untransformed pixels, drawn about `pivot` and supersampled
/// per canvas pixel. The samples are a sphere of radius half a pixel projected onto the
/// canvas, so a straight edge at any angle covers them linearly across one pixel, as
/// analytic antialiasing does, and any difference comes from the distance estimate.
fn supersampled_alpha(
    size: usize,
    pivot: (f64, f64),
    scale: (f64, f64),
    rotation: f64,
    inside: fn(f64, f64) -> bool,
) -> Vec<u8> {
    const RINGS: usize = 32;
    const ANGLES: usize = 16;
    let golden_angle = PI * (3.0 - 5f64.sqrt());
    let offsets: Vec<(f64, f64)> = (0..RINGS * ANGLES)
        .map(|sample| {
            let (ring, angle) = (sample / ANGLES, sample % ANGLES);
            let height = (ring as f64 + 0.5) / RINGS as f64;
            let radius = 0.5 * (1.0 - height * height).sqrt();
            let angle = TAU * (angle as f64 + 0.5) / ANGLES as f64 + ring as f64 * golden_angle;
            (0.5 + radius * angle.cos(), 0.5 + radius * angle.sin())
        })
        .collect();
    let inside_canvas = |point: (f64, f64)| {
        let (x, y) = drawable_to_local(point, pivot, scale, rotation);
        inside(x, y)
    };
    (0..size * size)
        .map(|index| {
            let origin = ((index % size) as f64, (index / size) as f64);
            let centre = inside_canvas((origin.0 + 0.5, origin.1 + 0.5));
            // Pixels whose surroundings out to a pixel agree with their centre are uniform.
            let uniform = (0..16).all(|step| {
                let angle = TAU * step as f64 / 16.0;
                inside_canvas((origin.0 + 0.5 + angle.cos(), origin.1 + 0.5 + angle.sin()))
                    == centre
            });
            if uniform {
                return if centre { 255 } else { 0 };
            }
            let covered = offsets
                .iter()
                .filter(|(dx, dy)| inside_canvas((origin.0 + dx, origin.1 + dy)))
                .count();
            (covered as f64 * 255.0 / offsets.len() as f64).round() as u8
        })
        .collect()
}

// Outlines in untransformed pixels. The shapes are centred on (70.3, 60.2).
fn capsule(x: f64, y: f64) -> bool {
    (x - x.clamp(60.0, 90.0)).hypot(y - 80.0) <= 2.0
}

fn stroked_rectangle(x: f64, y: f64) -> bool {
    let (x, y) = ((x - 70.3).abs(), (y - 60.2).abs());
    (x - 20.0).max(0.0).hypot((y - 20.0).max(0.0)) <= 2.0 && !(x < 18.0 && y < 18.0)
}

fn rounded_rectangle(x: f64, y: f64) -> bool {
    let (x, y) = ((x - 70.3).abs(), (y - 60.2).abs());
    (x - 5.0).max(0.0).hypot((y - 5.0).max(0.0)) <= 15.0
}

fn ellipse_outline(x: f64, y: f64) -> bool {
    let (x, y) = ((x - 70.3).abs(), (y - 60.2).abs());
    (x / 20.0).powi(2) + (y / 10.0).powi(2) <= 1.0
}

/// A shape or path drawn at `scale` and `rotation` against its supersampled outline.
fn outline_rows() -> Vec<Case> {
    const SIZE: usize = 256;
    let square = json!({
        "width": 40, "height": 40, "position": [50.3, 40.2], "fill_paint": WHITE,
        "stroke_paint": WHITE
    });
    // Sharp corners have no distance gradient, so analytic antialiasing cannot match area
    // coverage within a pixel of them. Under scale (4, 1) the stroke's ends and outer corners
    // curve with a radius of half a canvas pixel, where even the exact distance differs from
    // coverage by about 28, so those rows get a looser tolerance.
    let inner_corners: Vec<(f64, f64)> =
        [(-18.0, -18.0), (18.0, -18.0), (-18.0, 18.0), (18.0, 18.0)]
            .map(|(x, y)| (70.3 + x, 60.2 + y))
            .to_vec();

    let mut rows = Vec::new();
    for (scale, rotation) in [((1.0, 1.0), 0.0), ((4.0, 1.0), 0.0), ((4.0, 1.0), 25.0)] {
        let stroke_tolerance = if scale == (1.0, 1.0) { 10 } else { 32 };
        let transform = json!({ "scale": [scale.0, scale.1], "rotation": rotation });
        let drawn = [
            (
                "path caps",
                path(
                    (SIZE as u32, SIZE as u32),
                    with(
                        json!({
                            "data": "M5 20 L35 20", "position": [55, 60], "anchor": [20, 20],
                            "fill_enabled": false, "stroke_enabled": true, "stroke_width": 4,
                            "stroke_paint": WHITE
                        }),
                        transform.clone(),
                    ),
                ),
                (75.0, 80.0),
                capsule as fn(f64, f64) -> bool,
                Vec::new(),
                stroke_tolerance,
            ),
            (
                "stroked rectangle",
                shape(
                    (SIZE as u32, SIZE as u32),
                    with(
                        with(
                            square.clone(),
                            json!({ "fill_enabled": false, "stroke_enabled": true, "stroke_width": 4 }),
                        ),
                        transform.clone(),
                    ),
                ),
                (50.3, 40.2),
                stroked_rectangle,
                inner_corners.clone(),
                stroke_tolerance,
            ),
            (
                "rounded rectangle",
                shape(
                    (SIZE as u32, SIZE as u32),
                    with(
                        with(square.clone(), json!({ "border_radius": 15 })),
                        transform.clone(),
                    ),
                ),
                (50.3, 40.2),
                rounded_rectangle,
                Vec::new(),
                8,
            ),
            (
                "ellipse",
                shape(
                    (SIZE as u32, SIZE as u32),
                    with(
                        with(
                            square.clone(),
                            json!({
                                "geometry_kind": "ellipse", "height": 20, "position": [50.3, 50.2]
                            }),
                        ),
                        transform.clone(),
                    ),
                ),
                (50.3, 50.2),
                ellipse_outline,
                Vec::new(),
                12,
            ),
        ];
        for (name, drawing, pivot, inside, sharp_corners, tolerance) in drawn {
            let inner_corner_pixel =
                name == "stroked rectangle" && scale == (4.0, 1.0) && rotation == 0.0;
            rows.push(Case::new(
                format!("{name} at scale {scale:?}, {rotation} degrees"),
                drawing,
                Expect::Custom(Box::new(move |frames| {
                    let reference = supersampled_alpha(SIZE, pivot, scale, rotation, inside);
                    let near_sharp_corner = |x: usize, y: usize| {
                        let centre = (x as f64 + 0.5, y as f64 + 0.5);
                        sharp_corners.iter().any(|&corner| {
                            let (x, y) = drawable_to_local(centre, pivot, scale, rotation);
                            let local = (x - corner.0, y - corner.1);
                            // Back to canvas pixels about the corner.
                            let (sin, cos) = rotation.to_radians().sin_cos();
                            let (dx, dy) = (local.0 * scale.0, local.1 * scale.1);
                            (cos * dx - sin * dy).hypot(sin * dx + cos * dy) < 1.5
                        })
                    };
                    assert!(reference.contains(&255), "the outline is off the canvas");
                    let (error, (x, y)) = max_alpha_error_where(
                        &frames[0],
                        |x, y| reference[y * SIZE + x],
                        |x, y| !near_sharp_corner(x, y),
                    );
                    assert!(error <= tolerance, "alpha differs by {error} at ({x}, {y})");
                    if inner_corner_pixel {
                        assert_eq!(alpha_of(&frames[0], 54, 43), 255, "inside corner");
                    }
                })),
            ));
        }
    }
    rows
}

/// Like `max_alpha_error`, over the pixels `include` accepts.
fn max_alpha_error_where(
    frame: &Frame,
    expected: impl Fn(usize, usize) -> u8,
    include: impl Fn(usize, usize) -> bool,
) -> (u8, (usize, usize)) {
    let width = frame.width as usize;
    let (error, index) = pixels(frame)
        .enumerate()
        .filter(|(index, _)| include(index % width, index / width))
        .map(|(index, pixel)| {
            (
                pixel[3].abs_diff(expected(index % width, index / width)),
                index,
            )
        })
        .max()
        .unwrap();
    (error, (index % width, index / width))
}

/// Signed distance from `(x, y)` to the ellipse with radii `(a, b)` about the origin. The
/// nearest point is `(a² x / (t + a²), b² y / (t + b²))` for the root `t` of the function
/// below, which falls from infinity to -1 as `t` rises from `-min(a, b)²`.
fn ellipse_distance(x: f64, y: f64, a: f64, b: f64) -> f64 {
    let (x, y) = (x.abs().max(1e-9), y.abs().max(1e-9));
    let nearest = |t: f64| (a * a * x / (t + a * a), b * b * y / (t + b * b));
    let (mut low, mut high) = (-a.min(b).powi(2), a.max(b) * x.hypot(y));
    for _ in 0..80 {
        let t = 0.5 * (low + high);
        let (px, py) = nearest(t);
        if (px / a).powi(2) + (py / b).powi(2) > 1.0 {
            low = t;
        } else {
            high = t;
        }
    }
    let (px, py) = nearest(0.5 * (low + high));
    (x - px).hypot(y - py) * if low < 0.0 { -1.0 } else { 1.0 }
}

/// A 256 × 256 row whose alpha at each pixel centre must follow the exact canvas distance ramp,
/// so that any difference comes from the shader's distance.
fn exact_distance(
    name: &str,
    drawing: Scene,
    distance: impl Fn(f64, f64) -> f64 + 'static,
) -> Case {
    Case::new(
        name,
        drawing,
        Expect::Custom(Box::new(move |frames| {
            let frame = &frames[0];
            let (error, (x, y)) = max_alpha_error(frame, |x, y| {
                let exact = (0.5 - distance(x as f64 + 0.5, y as f64 + 0.5)).clamp(0.0, 1.0);
                (exact * 255.0).round() as u8
            });
            assert!(has_alpha_above(frame, 64), "nothing is drawn");
            assert!(error <= 2, "alpha differs by {error} at ({x}, {y})");
        })),
    )
}

fn ellipse_distance_rows() -> Vec<Case> {
    const CANVAS: (u32, u32) = (256, 256);
    // Each ellipse has a tip, on its outline, at the centre of pixel (100, 100), and its body
    // runs along the row or column through it, so the neighbouring pixels are inside the
    // ellipse close to its major axis.
    let tip = |name: &str, width: u32, height: u32, position: [f64; 2], centre: (f64, f64)| {
        exact_distance(
            name,
            shape(
                CANVAS,
                json!({
                    "geometry_kind": "ellipse", "width": width, "height": height,
                    "position": position
                }),
            ),
            move |x, y| {
                ellipse_distance(
                    x - centre.0,
                    y - centre.1,
                    width as f64 / 2.0,
                    height as f64 / 2.0,
                )
            },
        )
    };
    // The right tip is at (99.5, 101.5), and pixel (100, 100) is 1.41 px from it, inside the 2.5
    // px half stroke.
    let stroked_tip = |position: [f64; 2], extra: Value| {
        shape(
            CANVAS,
            with(
                json!({
                    "geometry_kind": "ellipse", "width": 68797, "height": 1,
                    "position": position, "fill_enabled": false, "stroke_enabled": true
                }),
                extra,
            ),
        )
    };
    // Squashed vertically, a tip's falloff runs diagonally, so the antialiasing ramp must follow
    // the displacement rather than the tip's horizontal surface normal.
    let squashed_tip = |x: f64| {
        stroked_tip(
            [x, 99.9255],
            json!({ "anchor": [34398.5, 0.5], "scale": [1, 0.1], "stroke_width": 2.5 }),
        )
    };
    // Positions a hair apart put pixel (102, 100) on either side of the evolute cusp, where the
    // iteration used to collapse onto the centre and draw a stray opaque pixel.
    let near_circle = |x: f64| {
        let drawing = shape(
            CANVAS,
            json!({
                "geometry_kind": "ellipse", "width": 202, "height": 200, "position": [x, 0.5],
                "fill_enabled": false, "stroke_enabled": true, "stroke_width": 5
            }),
        );
        Case::new(
            format!("a near-circle stroke at x = {x} is empty near its centre"),
            drawing,
            Expect::Custom(Box::new(move |frames| {
                let frame = &frames[0];
                let width = frame.width as usize;
                assert!(
                    pixels(frame).any(|pixel| pixel[3] == 255),
                    "the ring is not drawn"
                );
                let stray: Vec<_> = pixels(frame)
                    .enumerate()
                    .filter(|&(index, pixel)| {
                        let (px, py) = ((index % width) as f64 + 0.5, (index / width) as f64 + 0.5);
                        let distance = ellipse_distance(px - (x + 101.0), py - 100.5, 101.0, 100.0);
                        distance < -10.0 && pixel[3] != 0
                    })
                    .map(|(index, _)| (index % width, index / width))
                    .collect();
                assert!(stray.is_empty(), "stray stroke pixels at {stray:?}");
            })),
        )
    };

    let wide_stroke = shape(
        CANVAS,
        json!({
            "geometry_kind": "ellipse", "width": 40, "height": 20, "position": [50.5, 40.5],
            "fill_enabled": false, "stroke_enabled": true, "stroke_width": 24,
            "stroke_paint": WHITE
        }),
    );
    // Scaled by (4, 0.1) about its start, the stroke is a band 0.4 canvas pixels tall from
    // x = 70.5 to 190.5, and each cap an ellipse with radii (8, 0.2).
    let compressed_caps = path(
        CANVAS,
        json!({
            "data": "M5 20 L35 20", "position": [50.5, 40.5], "anchor": [0, 20],
            "scale": [4, 0.1], "fill_enabled": false, "stroke_enabled": true,
            "stroke_width": 4, "stroke_paint": WHITE
        }),
    );
    // A sliver a millionth of a pixel wide covers at most half of the pixels it crosses and
    // nothing beyond its ends.
    let sliver = shape(
        CANVAS,
        json!({
            "geometry_kind": "ellipse", "width": 40, "height": 20, "position": [128.5, 128.5],
            "anchor": [20, 10], "scale": [0.000001, 1]
        }),
    );

    vec![
        // A stroke wider than the ellipse is tall covers its whole inside.
        exact_distance("wide ellipse stroke", wide_stroke, |x, y| {
            ellipse_distance(x - 70.5, y - 50.5, 20.0, 10.0).abs() - 12.0
        }),
        exact_distance(
            "path caps compressed to slivers",
            compressed_caps,
            |x, y| {
                let (x, y) = (x - 130.5, y - 60.5);
                let (bx, by) = (x.abs() - 60.0, y.abs() - 0.2);
                let band = bx.max(0.0).hypot(by.max(0.0)) + bx.max(by).min(0.0);
                let caps = ellipse_distance(x + 60.0, y, 8.0, 0.2).min(ellipse_distance(
                    x - 60.0,
                    y,
                    8.0,
                    0.2,
                ));
                band.min(caps)
            },
        ),
        Case::new(
            "a sliver covers at most half of the pixels it crosses",
            sliver,
            Expect::Custom(Box::new(|frames| {
                let frame = &frames[0];
                for (index, pixel) in pixels(frame).enumerate() {
                    let (x, y) = (index % 256, index / 256);
                    let limit = if x == 148 && (128..=148).contains(&y) {
                        128
                    } else {
                        0
                    };
                    assert!(pixel[3] <= limit, "alpha {} at ({x}, {y})", pixel[3]);
                }
                let centre = alpha_of(frame, 148, 138);
                assert!(
                    centre >= 100,
                    "the sliver's centre pixel has alpha {centre}"
                );
            })),
        ),
        tip("ellipse 2000x2", 2000, 2, [-1899.5, 99.5], (-899.5, 100.5)),
        tip(
            "ellipse 20000x100",
            20000,
            100,
            [-19899.5, 50.5],
            (-9899.5, 100.5),
        ),
        tip("ellipse 2x2000", 2, 2000, [99.5, -1899.5], (100.5, -899.5)),
        Case::new(
            "an eccentric ellipse's stroked tip keeps its corner pixels",
            stroked_tip([-68697.5, 101.0], json!({ "stroke_width": 5 })),
            Expect::Probes(vec![(100, 100, BLACK)]),
        ),
        Case::new(
            "a squashed stroked tip follows the displacement",
            stroked_tip(
                [-68697.5, 100.1],
                json!({ "anchor": [68797, 0.5], "scale": [1, 0.1], "stroke_width": 2.5 }),
            ),
            Expect::Probes(vec![(100, 100, [0, 0, 0, 121])]),
        )
        .tolerance(6),
        near_circle(-0.4889678955078125),
        near_circle(-0.48895263671875),
        near_circle(-0.488983154296875),
        // A thousandth of a pixel of motion must not flip the edge between two normals.
        Case::new(
            "a squashed tip at x = -68697.378",
            squashed_tip(-68697.378),
            Expect::Probes(vec![(100, 100, [0, 0, 0, 131])]),
        ),
        Case::new(
            "a squashed tip a thousandth of a pixel on",
            squashed_tip(-68697.379),
            Expect::Probes(vec![(100, 100, [0, 0, 0, 131])]),
        ),
    ]
}

fn gradient_columns() -> Vec<(u32, u32, [u8; 4])> {
    (0..8)
        .map(|x| {
            let t = (f64::from(x) + 0.5) / 8.0;
            let channel = |value: f64| (value * 255.0).round() as u8;
            (x, 1, [channel(1.0 - t), 0, channel(t), 255])
        })
        .collect()
}

fn sampling_rows() -> Vec<Case> {
    let gradient = |width: u32, height: u32| {
        json!({
            "type": "background", "id": 1, "params": {
                "paint": { "type": "linear_gradient", "stops": [[0, RED], [1, BLUE]] },
                "width": width, "height": height, "paint_supersample": false
            }
        })
    };
    let resized = |source: (u32, u32)| {
        scene(
            (8, 4),
            json!([
                gradient(source.0, source.1),
                { "id": 2, "type": "resize", "in": { "source": 1 }, "params": {
                    "x": 0, "y": 0, "width": 8, "height": 4, "mode": "stretch",
                    "sampling": "linear"
                } },
            ]),
        )
    };
    vec![
        Case::new(
            "a gradient is red to blue across its width",
            scene((8, 4), json!([gradient(8, 4)])),
            Expect::Probes(gradient_columns()),
        ),
        Case::new(
            "an identity transform with linear sampling reproduces the gradient",
            scene(
                (8, 4),
                json!([
                    gradient(8, 4),
                    { "id": 2, "type": "transform", "in": { "source": 1 }, "params": {
                        "sampling": "linear"
                    } },
                ]),
            ),
            Expect::Probes(gradient_columns()),
        ),
        Case::new(
            "an identity linear resize reproduces the gradient",
            resized((8, 4)),
            Expect::Probes(gradient_columns()),
        ),
        Case::new(
            "a linear upscale keeps the source's edges opaque",
            resized((4, 2)),
            Expect::AlphaRows(vec!["########"; 4]),
        ),
    ]
}

fn crop_and_resize_rows() -> Vec<Case> {
    let background = |width: u32, height: u32| {
        json!({
            "id": 1, "type": "background", "params": {
                "paint": RED, "width": width, "height": height, "paint_supersample": true
            }
        })
    };
    let crop =
        |params: Value| json!({ "id": 2, "type": "crop", "in": { "source": 1 }, "params": params });
    let resize = |source: u32, params: Value| json!({ "id": 3, "type": "resize", "in": { "source": source }, "params": params });
    let region = |x: u32, y: u32, width: Value, height: u32| json!({ "x": x, "y": y, "width": width, "height": height });
    let placed = |mode: &str| json!({ "x": 0, "y": 0, "width": 4, "height": 4, "mode": mode, "sampling": "linear" });

    vec![
        Case::new(
            "crop keeps its region in place",
            scene(
                (8, 4),
                json!([background(8, 4), crop(region(2, 1, json!(3), 2))]),
            ),
            Expect::AlphaRows(vec!["........", "..###...", "..###...", "........"]),
        ),
        Case::new(
            "crop follows an animated width",
            scene(
                (8, 4),
                json!([background(8, 4), crop(region(2, 1, json!("=frame"), 2))]),
            )
            .frames(6),
            Expect::PerFrame(vec![
                Expect::AlphaRows(vec!["........", "..##....", "..##....", "........"]),
                Expect::AlphaRows(vec!["........", "..#####.", "..#####.", "........"]),
            ]),
        )
        .frames([2.0, 5.0]),
        Case::new(
            "resize places its output",
            scene(
                (8, 4),
                json!([
                    background(4, 2),
                    resize(
                        1,
                        json!({
                            "x": 3, "y": 1, "width": 2, "height": 2, "mode": "stretch",
                            "sampling": "nearest"
                        })
                    ),
                ]),
            ),
            Expect::AlphaRows(vec!["........", "...##...", "...##...", "........"]),
        ),
        Case::new(
            "resize follows an animated width",
            scene(
                (8, 4),
                json!([
                    background(4, 2),
                    resize(
                        1,
                        json!({
                            "x": 3, "y": 1, "width": "=frame", "height": 2, "mode": "stretch",
                            "sampling": "nearest"
                        })
                    ),
                ]),
            )
            .frames(5),
            Expect::PerFrame(vec![
                Expect::AlphaRows(vec!["........", "...#....", "...#....", "........"]),
                Expect::AlphaRows(vec!["........", "...####.", "...####.", "........"]),
            ]),
        )
        .frames([1.0, 4.0]),
        Case::new(
            "resize fits the cropped region, not the source",
            scene(
                (8, 4),
                json!([
                    background(8, 4),
                    crop(region(2, 0, json!(4), 4)),
                    resize(2, placed("fit"))
                ]),
            ),
            Expect::AlphaRows(vec!["####...."; 4]),
        ),
        Case::new(
            "resize fills from the cropped region, not the source",
            scene(
                (8, 4),
                json!([
                    background(8, 4),
                    crop(region(0, 0, json!(2), 4)),
                    resize(
                        2,
                        json!({
                            "x": 0, "y": 0, "width": 2, "height": 4, "mode": "fill",
                            "sampling": "linear"
                        })
                    ),
                ]),
            ),
            Expect::AlphaRows(vec!["##......"; 4]),
        ),
        Case::new(
            "resize fits an animated crop region",
            scene(
                (8, 4),
                json!([
                    background(8, 4),
                    crop(region(0, 0, json!("=frame"), 4)),
                    resize(2, placed("fit")),
                ]),
            )
            .frames(5),
            // A 2 × 4 region is centred in the 4 × 4 box, and a 4 × 4 region fills it.
            Expect::PerFrame(vec![
                Expect::AlphaRows(vec![".##....."; 4]),
                Expect::AlphaRows(vec!["####...."; 4]),
            ]),
        )
        .frames([2.0, 4.0]),
        Case::new(
            "resize fits the cropped region through an opacity node",
            scene(
                (8, 4),
                json!([
                    background(8, 4),
                    crop(region(2, 0, json!(4), 4)),
                    { "id": 4, "type": "opacity", "in": { "source": 2 }, "params": { "opacity": 1 } },
                    resize(4, placed("fit")),
                ]),
            ),
            Expect::AlphaRows(vec!["####...."; 4]),
        ),
    ]
}

#[test]
fn shapes_and_paths_cover_the_pixels_they_describe() {
    check(shape_coverage());
}

#[test]
fn transforms_and_scaling_equal_drawing_the_geometry_directly() {
    check(transform_equivalence());
}

#[test]
fn outlines_match_supersampling_under_scale_and_rotation() {
    check(outline_rows());
}

#[test]
fn ellipse_alpha_follows_the_exact_distance() {
    check(ellipse_distance_rows());
}

#[test]
fn sampling_keeps_gradients_and_edges() {
    check(sampling_rows());
}

#[test]
fn crop_and_resize_place_and_size_their_output() {
    check(crop_and_resize_rows());
}
