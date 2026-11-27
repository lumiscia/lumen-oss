//! Drawings rendered in one pass by `render_2d`, against the same outlines each drawn by its own
//! `render_2d` and merged in order.

use serde_json::{Value, json};

use crate::support::{CLEAR, Case, Expect, Scene, check, filled_and_stroked, scene};

const SIZE: (u32, u32) = (160, 96);
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// An outline styled by fills and strokes: `kind` is `path_2d` or `shape_2d`, `geometry` its
/// outline and placement params, and each style the params of a `fill_2d` or `stroke_2d`.
struct Outline {
    kind: &'static str,
    geometry: Value,
    styles: Vec<(&'static str, Value)>,
}

/// The outlines as geometry nodes styled into one `render_2d`, each geometry shared by its
/// styles, and as one `render_2d` per style, each with its own geometry, in a
/// `raster_multimerge`.
fn both(outlines: &[Outline]) -> (Scene, Scene) {
    let mut together = Vec::new();
    let mut apart = Vec::new();
    let mut drawn = Vec::new();
    let mut next = 1;
    for outline in outlines {
        let geometry = next;
        next += 1;
        together.push(json!({ "id": geometry, "type": outline.kind, "params": outline.geometry }));
        for style in &outline.styles {
            together.push(json!({ "id": next, "type": style.0, "params": style.1, "in": { "geometry": geometry } }));
            apart.push(
                json!({ "id": next + 200, "type": outline.kind, "params": outline.geometry }),
            );
            apart.push(json!({ "id": next + 400, "type": style.0, "params": style.1, "in": { "geometry": next + 200 } }));
            apart.push(json!({ "id": next, "type": "render_2d", "in": { "drawing": next + 400 } }));
            drawn.push(next);
            next += 1;
        }
    }
    together.push(json!({ "id": 100, "type": "merge_2d", "in": { "drawings": drawn } }));
    together.push(json!({ "id": 101, "type": "render_2d", "in": { "drawing": 100 } }));
    apart.push(json!({ "id": 100, "type": "raster_multimerge", "in": { "layers": drawn } }));
    (scene(SIZE, json!(together)), scene(SIZE, json!(apart)))
}

fn matches_separate_renders(name: &str, outlines: &[Outline]) -> Case {
    let (together, apart) = both(outlines);
    Case::new(name, together, Expect::Matches(apart.into()))
}

const CIRCLE: &str = "M 0 -20 C 11 -20 20 -11 20 0 C 20 11 11 20 0 20 C -11 20 -20 11 -20 0 C -20 -11 -11 -20 0 -20 Z";

fn fills_and_strokes_match_separate_renders() -> Vec<Case> {
    vec![
        // The paths share the points and bands buffers, so each must read its own; the triangle's
        // fill and stroke share one outline.
        matches_separate_renders(
            "fills and strokes of paths draw as each drawn alone",
            &[
                Outline {
                    kind: "path_2d",
                    geometry: json!({ "data": CIRCLE, "position": [30, 48] }),
                    styles: vec![(
                        "stroke_2d",
                        json!({ "paint": [40, 200, 255, 255], "width": 4, "dash": [6, 3], "dash_offset": 2 }),
                    )],
                },
                Outline {
                    kind: "path_2d",
                    geometry: json!({ "data": CIRCLE, "position": [80, 48], "rotation": 30 }),
                    styles: vec![(
                        "stroke_2d",
                        json!({ "paint": [40, 200, 255, 255], "width": 4, "trim_start": 0.1, "trim_end": 0.7 }),
                    )],
                },
                Outline {
                    kind: "path_2d",
                    geometry: json!({
                        "data": "M 0 0 L 30 6 L 8 28 Z", "position": [112, 30], "anchor": [12, 12],
                        "rotation": -15, "scale": [1.3, 0.8]
                    }),
                    styles: vec![
                        (
                            "fill_2d",
                            json!({ "paint": [255, 120, 0, 200], "opacity": 0.8, "feather": 3 }),
                        ),
                        ("stroke_2d", json!({ "width": 2, "opacity": 0.8 })),
                    ],
                },
            ],
        ),
        // The square covers whole tiles, which list nothing below it, and part of the tiles along
        // its edges, where the stroke below still shows.
        matches_separate_renders(
            "an opaque path fill hides only what lies under it",
            &[
                Outline {
                    kind: "path_2d",
                    geometry: json!({ "data": "M 0 48 L 160 40", "position": [0, 0] }),
                    styles: vec![(
                        "stroke_2d",
                        json!({ "paint": [255, 0, 0, 255], "width": 6 }),
                    )],
                },
                Outline {
                    kind: "path_2d",
                    geometry: json!({
                        "data": "M 0 0 H 90 V 90 H 0 Z", "position": [80, 48], "anchor": [45, 45],
                        "rotation": 10
                    }),
                    styles: vec![("fill_2d", json!({ "paint": [0, 160, 255, 255] }))],
                },
            ],
        ),
        matches_separate_renders(
            "fills and strokes of shapes draw as each drawn alone, the last on top",
            &[
                Outline {
                    kind: "shape_2d",
                    geometry: json!({ "width": 70, "height": 40, "position": [16, 20], "border_radius": 10 }),
                    styles: vec![
                        ("fill_2d", json!({ "paint": [255, 0, 0, 160] })),
                        (
                            "stroke_2d",
                            json!({ "paint": [255, 255, 255, 255], "width": 3 }),
                        ),
                    ],
                },
                Outline {
                    kind: "shape_2d",
                    geometry: json!({
                        "geometry_kind": "ellipse", "width": 60, "height": 50, "position": [60, 30],
                        "anchor": [30, 25], "rotation": 25
                    }),
                    styles: vec![
                        (
                            "fill_2d",
                            json!({ "paint": [0, 80, 255, 200], "opacity": 0.7 }),
                        ),
                        ("stroke_2d", json!({ "paint": [0, 0, 0, 255], "width": 2 })),
                    ],
                },
            ],
        ),
    ]
}

/// A 40 pixel square at (40, 28), its outline grown or shrunk by `offset`, styled by `style`.
fn offset_square(offset: f64, style: (&str, Value)) -> Scene {
    scene(
        SIZE,
        json!([
            { "id": 1, "type": "path_2d", "params": { "data": "M 0 0 H 40 V 40 H 0 Z", "position": [40, 28] } },
            { "id": 2, "type": "offset_2d", "params": { "offset": offset / 2.0 }, "in": { "geometry": 1 } },
            { "id": 3, "type": "offset_2d", "params": { "offset": offset / 2.0 }, "in": { "geometry": 2 } },
            { "id": 4, "type": style.0, "params": style.1, "in": { "geometry": 3 } },
            { "id": 5, "type": "render_2d", "in": { "drawing": 4 } }
        ]),
    )
}

/// Chained offsets add up, and both the fill and the stroke follow the outline they leave.
fn offsets_move_fill_and_stroke_edges() -> Vec<Case> {
    let fill = || ("fill_2d", json!({}));
    let stroke = || {
        (
            "stroke_2d",
            json!({ "paint": [255, 255, 255, 255], "width": 4 }),
        )
    };
    vec![
        Case::new(
            "a fill grown twice by 4 draws as one grown by 8",
            offset_square(8.0, fill()),
            Expect::Matches(
                scene(
                    SIZE,
                    json!([
                        { "id": 1, "type": "path_2d", "params": { "data": "M 0 0 H 40 V 40 H 0 Z", "position": [40, 28] } },
                        { "id": 2, "type": "offset_2d", "params": { "offset": 8 }, "in": { "geometry": 1 } },
                        { "id": 3, "type": "fill_2d", "in": { "geometry": 2 } },
                        { "id": 4, "type": "render_2d", "in": { "drawing": 3 } }
                    ]),
                )
                .into(),
            ),
        ),
        // Grown by 8, the square's outline is a rounded rectangle with corners of radius 8.
        Case::new(
            "a grown stroke follows the rounded outline",
            offset_square(8.0, stroke()),
            Expect::Matches(
                scene(
                    SIZE,
                    json!([
                        { "id": 1, "type": "shape_2d", "params": {
                            "width": 56, "height": 56, "position": [32, 20], "border_radius": 8
                        } },
                        { "id": 2, "type": "stroke_2d", "params": { "paint": [255, 255, 255, 255], "width": 4 }, "in": { "geometry": 1 } },
                        { "id": 3, "type": "render_2d", "in": { "drawing": 2 } }
                    ]),
                )
                .into(),
            ),
        ),
        // Shrunk by 6, the stroke covers 4 to 8 pixels inside the square's edges.
        Case::new(
            "a shrunk stroke lies inside the outline",
            offset_square(-6.0, stroke()),
            Expect::Probes(vec![
                (46, 48, WHITE),
                (40, 48, CLEAR),
                (42, 48, CLEAR),
                (50, 48, CLEAR),
                (60, 48, CLEAR),
                (60, 34, WHITE),
                (73, 48, WHITE),
            ]),
        ),
    ]
}

/// Two squares, the second over the first, cast a hard shadow down and to the right: the
/// second's shadow lies under the first square, not over it.
fn shadows_draw_beneath_the_whole_drawing() -> Case {
    let square = |id: u32, position: [u32; 2]| json!({ "id": id, "type": "shape_2d", "params": { "width": 30, "height": 30, "position": position } });
    Case::new(
        "a shadow lies beneath every drawable of its drawing",
        scene(
            SIZE,
            json!([
                square(1, [60, 20]),
                square(2, [20, 20]),
                { "id": 3, "type": "fill_2d", "params": { "paint": [255, 255, 255, 255] }, "in": { "geometry": 1 } },
                { "id": 4, "type": "fill_2d", "params": { "paint": [255, 0, 0, 255] }, "in": { "geometry": 2 } },
                { "id": 5, "type": "merge_2d", "in": { "drawings": [3, 4] } },
                {
                    "id": 6, "type": "shadow_2d", "in": { "drawing": 5 },
                    "params": { "shadows": [{ "color": [0, 0, 0, 255], "offset": [44, 10], "radius": 0, "opacity": 1 }] }
                },
                { "id": 7, "type": "render_2d", "in": { "drawing": 6 } }
            ]),
        ),
        Expect::Probes(vec![
            // The red square's shadow reaches under the white square.
            (70, 40, WHITE),
            (30, 30, [255, 0, 0, 255]),
            (70, 55, BLACK),
            (110, 45, BLACK),
            (97, 45, CLEAR),
            (140, 45, CLEAR),
            (55, 10, CLEAR),
        ]),
    )
}

/// A feathered square fades the same whether it is a shape or a path.
fn feathered_shapes_fade_as_paths_do() -> Case {
    let fill = |geometry: Value| {
        scene(
            SIZE,
            json!([
                geometry,
                { "id": 2, "type": "fill_2d", "params": { "paint": [255, 255, 255, 255], "feather": 12 }, "in": { "geometry": 1 } },
                { "id": 3, "type": "render_2d", "in": { "drawing": 2 } }
            ]),
        )
    };
    let shape = fill(
        json!({ "id": 1, "type": "shape_2d", "params": { "width": 40, "height": 40, "position": [60, 28] } }),
    );
    let path = fill(
        json!({ "id": 1, "type": "path_2d", "params": { "data": "M 0 0 H 40 V 40 H 0 Z", "position": [60, 28] } }),
    );
    Case::new(
        "a feathered shape fades as the same path does",
        shape,
        Expect::Matches(path.into()),
    )
}

/// Transparent fills of a shape and a path cast no shadow, where an opaque one beside them does.
fn transparent_fills_cast_no_shadow() -> Case {
    let fill = |id: u32, geometry: u32, paint: [u8; 4]| json!({ "id": id, "type": "fill_2d", "params": { "paint": paint }, "in": { "geometry": geometry } });
    Case::new(
        "a shadow is as opaque as the paint it falls from",
        scene(
            SIZE,
            json!([
                { "id": 1, "type": "shape_2d", "params": { "width": 30, "height": 30, "position": [10, 10] } },
                { "id": 2, "type": "path_2d", "params": { "data": "M 0 0 H 30 V 30 H 0 Z", "position": [70, 10] } },
                { "id": 3, "type": "shape_2d", "params": { "width": 20, "height": 30, "position": [130, 10] } },
                fill(4, 1, [255, 0, 0, 0]),
                fill(5, 2, [255, 0, 0, 0]),
                fill(6, 3, [255, 255, 255, 255]),
                { "id": 7, "type": "merge_2d", "in": { "drawings": [4, 5, 6] } },
                {
                    "id": 8, "type": "shadow_2d", "in": { "drawing": 7 },
                    "params": { "shadows": [{ "color": [0, 0, 0, 255], "offset": [0, 45], "radius": 0, "opacity": 1 }] }
                },
                { "id": 9, "type": "render_2d", "in": { "drawing": 8 } }
            ]),
        ),
        Expect::Probes(vec![(25, 70, CLEAR), (85, 70, CLEAR), (140, 70, BLACK)]),
    )
}

/// Columns of a rotation by `angle` radians, and their product with a vector.
fn rotation(angle: f64) -> [[f64; 2]; 2] {
    let (sin, cos) = angle.sin_cos();
    [[cos, sin], [-sin, cos]]
}

fn apply(m: [[f64; 2]; 2], v: [f64; 2]) -> [f64; 2] {
    [
        m[0][0] * v[0] + m[1][0] * v[1],
        m[0][1] * v[0] + m[1][1] * v[1],
    ]
}

/// A merge with non-uniform scale around a rotated path shears it. The same canvas outline is a
/// `path_2d` whose points are turned first, then scaled and rotated: the map's singular value
/// decomposition. Its stroke is a band of the same untransformed width around the same points.
fn sheared_merges_draw_as_turned_paths() -> Case {
    let points = [[-20.0, -12.0], [22.0, -8.0], [14.0, 16.0], [-16.0, 10.0]];
    let data = |points: &[[f64; 2]]| {
        let mut data = String::new();
        for (index, [x, y]) in points.iter().enumerate() {
            data += &format!("{} {x} {y} ", if index == 0 { "M" } else { "L" });
        }
        data + "Z"
    };
    // `scale(2, 1) * rotation(30 degrees)`, decomposed as `rotation(after) * scale * rotation(turn)`.
    let r = rotation(30f64.to_radians());
    let [[a, c], [b, d]] = [[2.0 * r[0][0], r[0][1]], [2.0 * r[1][0], r[1][1]]];
    let (e, f, g, h) = ((a + d) / 2.0, (a - d) / 2.0, (c + b) / 2.0, (c - b) / 2.0);
    let (q, s) = (f64::hypot(e, h), f64::hypot(f, g));
    let (a1, a2) = (g.atan2(f), h.atan2(e));
    let turn = rotation((a2 - a1) / 2.0);
    let turned: Vec<[f64; 2]> = points.iter().map(|&point| apply(turn, point)).collect();
    let reference = json!({
        "data": data(&turned), "position": [80, 48], "scale": [q + s, q - s],
        "rotation": ((a2 + a1) / 2.0).to_degrees()
    });
    Case::new(
        "a sheared merge draws its path as the turned path",
        scene(
            SIZE,
            json!([
                { "id": 1, "type": "path_2d", "params": { "data": data(&points), "rotation": 30 } },
                { "id": 2, "type": "fill_2d", "params": { "paint": [255, 200, 0, 255] }, "in": { "geometry": 1 } },
                { "id": 3, "type": "stroke_2d", "params": { "width": 3 }, "in": { "geometry": 1 } },
                { "id": 4, "type": "merge_2d", "params": { "position": [80, 48], "scale": [2, 1] }, "in": { "drawings": [2, 3] } },
                { "id": 5, "type": "render_2d", "in": { "drawing": 4 } }
            ]),
        ),
        Expect::Matches(
            scene(
                SIZE,
                json!(filled_and_stroked(
                    1,
                    "path_2d",
                    reference,
                    json!({ "paint": [255, 200, 0, 255] }),
                    json!({ "width": 3 }),
                )),
            )
            .into(),
        ),
    )
}

#[test]
fn render_2d() {
    let mut cases = fills_and_strokes_match_separate_renders();
    cases.extend(offsets_move_fill_and_stroke_edges());
    cases.push(shadows_draw_beneath_the_whole_drawing());
    cases.push(sheared_merges_draw_as_turned_paths());
    cases.push(feathered_shapes_fade_as_paths_do());
    cases.push(transparent_fills_cast_no_shadow());
    check(cases);
}
