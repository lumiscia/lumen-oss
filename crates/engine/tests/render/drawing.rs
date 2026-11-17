//! Drawings rendered in one pass by `render_2d`, against the same outlines drawn by `path` and
//! `shape` nodes merged in order.

use serde_json::{Map, Value, json};

use crate::support::{CLEAR, Case, Expect, Scene, check, scene};

const SIZE: (u32, u32) = (160, 96);
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// An outline styled by fills and strokes: `kind` is `path` or `shape`, `geometry` its outline
/// and placement params, and each style the params of a `fill_2d` or `stroke_2d`.
struct Outline {
    kind: &'static str,
    geometry: Value,
    styles: Vec<(&'static str, Value)>,
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().expect("an object").clone()
}

/// The params of a `path` or `shape` node drawing `style` of `outline`.
fn raster_params(outline: &Outline, (style, params): &(&str, Value)) -> Value {
    let mut raster = object(outline.geometry.clone());
    let params = object(params.clone());
    let get = |name: &str| params.get(name).cloned();
    let fill = *style == "fill_2d";
    raster.insert("fill_enabled".into(), json!(fill));
    raster.insert("stroke_enabled".into(), json!(!fill));
    if let Some(paint) = get("paint") {
        raster.insert(
            if fill { "fill_paint" } else { "stroke_paint" }.into(),
            paint,
        );
    }
    for (from, to) in [
        ("opacity", "opacity"),
        ("feather", "feather"),
        ("width", "stroke_width"),
        ("trim_start", "trim_start"),
        ("trim_end", "trim_end"),
        ("dash", "stroke_dash"),
        ("dash_offset", "stroke_dash_offset"),
    ] {
        if let Some(value) = get(from) {
            raster.insert(to.into(), value);
        }
    }
    Value::Object(raster)
}

/// The outlines as geometry nodes styled into one `render_2d`, each geometry shared by its
/// styles, and as one `path` or `shape` node per style in a `raster_multimerge`.
fn both(outlines: &[Outline]) -> (Scene, Scene) {
    let mut vector = Vec::new();
    let mut raster = Vec::new();
    let mut drawn = Vec::new();
    let mut next = 1;
    for outline in outlines {
        let geometry = next;
        next += 1;
        vector.push(json!({ "id": geometry, "type": format!("{}_2d", outline.kind), "params": outline.geometry }));
        for style in &outline.styles {
            vector.push(json!({ "id": next, "type": style.0, "params": style.1, "in": { "geometry": geometry } }));
            raster.push(json!({ "id": next, "type": outline.kind, "params": raster_params(outline, style) }));
            drawn.push(next);
            next += 1;
        }
    }
    vector.push(json!({ "id": 100, "type": "merge_2d", "in": { "drawings": drawn } }));
    vector.push(json!({ "id": 101, "type": "render_2d", "in": { "drawing": 100 } }));
    raster.push(json!({ "id": 100, "type": "raster_multimerge", "in": { "layers": drawn } }));
    (scene(SIZE, json!(vector)), scene(SIZE, json!(raster)))
}

fn matches_raster_nodes(name: &str, outlines: &[Outline]) -> Case {
    let (vector, raster) = both(outlines);
    Case::new(name, vector, Expect::Matches(raster.into()))
}

const CIRCLE: &str = "M 0 -20 C 11 -20 20 -11 20 0 C 20 11 11 20 0 20 C -11 20 -20 11 -20 0 C -20 -11 -11 -20 0 -20 Z";

fn fills_and_strokes_match_raster_nodes() -> Vec<Case> {
    vec![
        // The paths share the points and bands buffers, so each must read its own; the triangle's
        // fill and stroke share one outline.
        matches_raster_nodes(
            "fills and strokes of paths draw as path nodes",
            &[
                Outline {
                    kind: "path",
                    geometry: json!({ "data": CIRCLE, "position": [30, 48] }),
                    styles: vec![(
                        "stroke_2d",
                        json!({ "paint": [40, 200, 255, 255], "width": 4, "dash": [6, 3], "dash_offset": 2 }),
                    )],
                },
                Outline {
                    kind: "path",
                    geometry: json!({ "data": CIRCLE, "position": [80, 48], "rotation": 30 }),
                    styles: vec![(
                        "stroke_2d",
                        json!({ "paint": [40, 200, 255, 255], "width": 4, "trim_start": 0.1, "trim_end": 0.7 }),
                    )],
                },
                Outline {
                    kind: "path",
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
        matches_raster_nodes(
            "an opaque path fill hides only what lies under it",
            &[
                Outline {
                    kind: "path",
                    geometry: json!({ "data": "M 0 48 L 160 40", "position": [0, 0] }),
                    styles: vec![(
                        "stroke_2d",
                        json!({ "paint": [255, 0, 0, 255], "width": 6 }),
                    )],
                },
                Outline {
                    kind: "path",
                    geometry: json!({
                        "data": "M 0 0 H 90 V 90 H 0 Z", "position": [80, 48], "anchor": [45, 45],
                        "rotation": 10
                    }),
                    styles: vec![("fill_2d", json!({ "paint": [0, 160, 255, 255] }))],
                },
            ],
        ),
        matches_raster_nodes(
            "fills and strokes of shapes draw as shape nodes, the last on top",
            &[
                Outline {
                    kind: "shape",
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
                    kind: "shape",
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
            "a grown fill draws as a path node's offset fill",
            offset_square(8.0, fill()),
            Expect::Matches(
                scene(
                    SIZE,
                    json!([{ "id": 1, "type": "path", "params": {
                        "data": "M 0 0 H 40 V 40 H 0 Z", "position": [40, 28], "offset": 8
                    } }]),
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
                    json!([{ "id": 1, "type": "shape", "params": {
                        "width": 56, "height": 56, "position": [32, 20], "border_radius": 8,
                        "fill_enabled": false, "stroke_enabled": true, "stroke_width": 4,
                        "stroke_paint": [255, 255, 255, 255]
                    } }]),
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
/// `path` node whose points are turned first, then scaled and rotated: the map's singular value
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
    let style =
        json!({ "fill_paint": [255, 200, 0, 255], "stroke_enabled": true, "stroke_width": 3 });
    let mut reference = object(style);
    reference.extend(object(json!({
        "data": data(&turned), "position": [80, 48], "scale": [q + s, q - s],
        "rotation": ((a2 + a1) / 2.0).to_degrees()
    })));
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
                json!([{ "id": 1, "type": "path", "params": reference }]),
            )
            .into(),
        ),
    )
}

#[test]
fn render_2d() {
    let mut cases = fills_and_strokes_match_raster_nodes();
    cases.extend(offsets_move_fill_and_stroke_edges());
    cases.push(shadows_draw_beneath_the_whole_drawing());
    cases.push(sheared_merges_draw_as_turned_paths());
    check(cases);
}
