//! What a frame binds: the errors it reports, the bounds it records and the glyphs it places.

use lumen_engine::{composition::Composition, node::NodeId};
use lumen_gpu::{RectI, Size};
use serde_json::{Value, json};

use crate::support::{Case, Expect, TestMedia, check, gpu, render, scene};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn composition((width, height): (u32, u32), nodes: Value) -> Composition {
    scene((width, height), nodes).frames(12).into()
}

fn solid(id: u64, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "background", "params": {
        "paint": color, "width": 8, "height": 4
    } })
}

struct BindError {
    name: String,
    composition: Composition,
    message: String,
}

fn row(name: &str, nodes: Value, message: impl Into<String>) -> BindError {
    BindError {
        name: name.to_string(),
        composition: composition((8, 4), nodes),
        message: message.into(),
    }
}

fn text(content: &str, params: Value) -> Value {
    let mut node = json!({ "id": 1, "type": "text", "params": { "content": content } });
    for (key, value) in params.as_object().unwrap() {
        node["params"][key] = value.clone();
    }
    node
}

#[test]
fn bind_errors() {
    let property = |node: u64, path: &str| format!("property `{path}` on node {node} ");
    let glyph = |name: &str| format!("`{name}` is only available in a text node's glyph_offset");
    let crop_resize = |crop: [i64; 4], resize: [i64; 4]| {
        let [x, y, width, height] = crop;
        let [resize_x, resize_y, resize_width, resize_height] = resize;
        json!([
            solid(1, [255, 0, 0, 255]),
            { "id": 2, "type": "crop", "in": { "source": 1 },
              "params": { "x": x, "y": y, "width": width, "height": height } },
            { "id": 3, "type": "resize", "in": { "source": 2 }, "params": {
                "x": resize_x, "y": resize_y, "width": resize_width, "height": resize_height,
                "mode": "stretch", "sampling": "linear"
            } },
        ])
    };

    let mut rows = Vec::new();
    // Origins are `i32` and extents `u32` on the GPU.
    let valid = [0, 0, 4, 4];
    for (index, path) in ["x", "y", "width", "height"].into_iter().enumerate() {
        let mut invalid = valid;
        invalid[index] = 1 << 32;
        rows.push(row(
            &format!("crop {path} beyond the GPU range"),
            crop_resize(invalid, valid),
            property(2, path),
        ));
        rows.push(row(
            &format!("resize {path} beyond the GPU range"),
            crop_resize(valid, invalid),
            property(3, path),
        ));
    }
    for source in ["0,0,0; nan,1,1", "0,0,0; inf,1,1", "0,0,0; 1e99,1,1"] {
        rows.push(row(
            &format!("color_grade LUT `{source}`"),
            json!([
                solid(1, [255, 0, 0, 255]),
                { "id": 2, "type": "color_grade", "in": { "source": 1 },
                  "params": { "lut_source": source } },
            ]),
            "ColorGrade node 2 evaluation failed: LUT components must be finite",
        ));
    }
    rows.push(row(
        "crop origin below i32::MIN",
        crop_resize([i64::from(i32::MIN) - 1, 0, 4, 4], valid),
        property(2, "x"),
    ));
    rows.push(row(
        "word outside glyph params",
        json!([text("Lumen", json!({ "position": "=vec2(word * 10, 0)" }))]),
        glyph("word"),
    ));
    rows.push(row(
        "line outside glyph params",
        json!([text("Lumen", json!({ "opacity": "=line" }))]),
        glyph("line"),
    ));
    rows.push(row(
        "glyph on a node that is not text",
        json!([{ "id": 1, "type": "shape", "params": { "opacity": "=glyph / glyph_count" } }]),
        glyph("glyph"),
    ));
    // Another node's property is evaluated outside the glyph, so it fails as it would alone.
    let foreign = |glyph_offset: &str| {
        json!([
            text("Lumen", json!({ "glyph_offset": glyph_offset })),
            { "id": 2, "type": "text", "params": { "content": "Lumen", "font_size": "=20 + glyph * 10" } },
            { "id": 3, "type": "exposure", "in": { "source": 1 } },
        ])
    };
    rows.push(row(
        "foreign property read in glyph scope",
        foreign("=vec2(node(2, \"font_size\"), 0)"),
        "`glyph` is only available",
    ));
    rows.push(row(
        "text_width of a foreign node in glyph scope",
        foreign("=vec2(text_width(node(2)), 0)"),
        "`glyph` is only available",
    ));
    // An expression can only name a value the enum has.
    let source = json!({ "source": 1 });
    for (kind, inputs, param, value, enum_name) in [
        (
            "boolean",
            json!({ "a": 1, "b": 2 }),
            "operation",
            "=4",
            "BooleanOperation",
        ),
        (
            "raster_multimerge",
            json!({ "layers": [1, 2] }),
            "blend_mode",
            "=6",
            "BlendMode",
        ),
        (
            "merge",
            json!({ "base": 1, "overlay": 2 }),
            "blend_mode",
            "=6",
            "BlendMode",
        ),
        (
            "color_grade",
            source.clone(),
            "interpolation",
            "=2",
            "RasterSampling",
        ),
        (
            "resize",
            source.clone(),
            "sampling",
            "=-1",
            "RasterSampling",
        ),
        ("transform", source, "sampling", "=2", "RasterSampling"),
    ] {
        rows.push(row(
            &format!("{kind} {param} outside its enum"),
            json!([
                solid(1, [255, 0, 0, 255]),
                solid(2, [0, 0, 255, 255]),
                { "id": 3, "type": kind, "in": inputs, "params": { param: value } },
            ]),
            format!("is not a value of {enum_name}"),
        ));
    }
    rows.push(row(
        "shadow color that is a gradient",
        json!([
            solid(1, [255, 0, 0, 255]),
            { "id": 2, "type": "shadow", "in": { "source": 1 }, "params": { "color": {
                "type": "linear_gradient",
                "stops": [
                    { "offset": 0.0, "color": [255, 0, 0, 255] },
                    { "offset": 1.0, "color": [0, 0, 255, 255] }
                ]
            } } },
        ]),
        "property `color` on node 2 has invalid type: expected a solid color, got a gradient",
    ));
    rows.push(row(
        "malformed path data",
        json!([{ "id": 1, "type": "path", "params": { "data": "M 0 0 L 10 10 C nope" } }]),
        "Path node 1 evaluation failed: `data` is malformed",
    ));
    let mut data = String::from("M 10 10");
    for index in 1..20_000 - 2 {
        data += &format!(
            " L {} {}",
            10 + index % 100,
            if index % 2 == 0 { 10 } else { 11 }
        );
    }
    rows.push(row(
        "path above the point limit",
        json!([{ "id": 1, "type": "path", "params": { "data": data + " Z" } }]),
        "more than 16384 points",
    ));

    rows.push(row(
        "media source that can't be resolved",
        json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "gone" } }]),
        "not registered",
    ));

    let gpu = gpu();
    let media = TestMedia::default();
    for row in rows {
        let composition = row.composition;
        let mut renderer = gpu.renderer();
        renderer.compile(&composition).unwrap();
        let error = renderer
            .bind_frame(&composition, 0, &media)
            .map(|_| ())
            .expect_err(&row.name)
            .to_string();
        assert!(error.contains(&row.message), "{}: {error}", row.name);
    }
}

/// Two frames on one renderer, so the second has to replace what the first wrote.
#[test]
fn animated_values_update_each_frame() {
    let color = [200, 100, 50, 255];
    let fading = json!({ "id": 2, "type": "opacity", "in": { "source": 1 },
        "params": { "opacity": "=frame / 10" } });
    let half = |id: u32, source: u32, x: u32| {
        json!({ "id": id, "type": "crop", "in": { "source": source },
            "params": { "x": x, "y": 0, "width": 4, "height": 4 } })
    };
    let halves =
        |left, right| Expect::PerFrame(vec![Expect::Probes(vec![(0, 0, left), (7, 3, right)])]);
    let row = |name, nodes, frames: [Expect; 2]| {
        Case::new(
            name,
            scene((8, 4), nodes).frames(12),
            Expect::PerFrame(frames.into()),
        )
        .frames([5.0, 10.0])
        .tolerance(1)
    };
    check(vec![
        row(
            "exposure",
            json!([
                solid(1, [50, 25, 10, 255]),
                { "id": 2, "type": "exposure", "in": { "source": 1 },
                  "params": { "exposure": "=frame / 5 - 1" } },
            ]),
            [
                Expect::Uniform([50, 25, 10, 255]),
                Expect::Uniform([100, 50, 20, 255]),
            ],
        ),
        row(
            "opacity",
            json!([solid(1, color), fading]),
            [Expect::Uniform([100, 50, 25, 128]), Expect::Uniform(color)],
        ),
        // The source is drawn at the frame on the left, and at the remapped frame 8 on the right.
        row(
            "a source read directly and through a remap",
            json!([
                solid(1, color),
                fading,
                half(3, 2, 0),
                { "id": 4, "type": "time_remap", "in": { "source": 2 }, "params": {
                    "frame": 8.0, "loop_enabled": false, "loop_start": 0, "loop_end": 0
                } },
                half(5, 4, 4),
                { "id": 6, "type": "merge", "in": { "base": 3, "overlay": 5 } },
            ]),
            [
                halves([100, 50, 25, 128], [160, 80, 40, 204]),
                halves(color, [160, 80, 40, 204]),
            ],
        ),
    ]);
}

/// Only the selected branch is drawn, so a frame records one pass for its one solid.
#[test]
fn routing_records_one_branch() {
    let layers = |selected: Value| {
        json!([
            solid(1, RED),
            solid(2, GREEN),
            { "id": 3, "type": "switch", "params": { "selected_layer": selected },
              "in": { "layers": [1, 2] } },
        ])
    };
    let frame_switch = layers(json!("=if(frame < 6, 0, 1)"));
    let rows = [
        ("literal switch", layers(json!(0)), 0.0, RED),
        ("frame switch, first branch", frame_switch.clone(), 0.0, RED),
        ("frame switch, second branch", frame_switch, 10.0, GREEN),
        (
            "time remap",
            json!([
                solid(1, RED),
                { "id": 2, "type": "time_remap", "params": {
                    "frame": 8.0, "loop_enabled": true, "loop_start": 4, "loop_end": 12
                }, "in": { "source": 1 } },
            ]),
            0.0,
            RED,
        ),
        (
            // The switch is evaluated at the remapped frame 0, not at the frame 10 being drawn.
            "remap evaluates its source at the remapped frame",
            json!([
                solid(1, RED),
                solid(2, GREEN),
                { "id": 3, "type": "switch", "params": { "selected_layer": "=if(frame < 6, 0, 1)" },
                  "in": { "layers": [1, 2] } },
                { "id": 4, "type": "time_remap", "params": {
                    "frame": 0.0, "loop_enabled": false, "loop_start": 0, "loop_end": 0
                }, "in": { "source": 3 } },
            ]),
            10.0,
            RED,
        ),
    ];
    let gpu = gpu();
    let media = TestMedia::default();
    for (name, nodes, frame, color) in rows {
        let mut renderer = gpu.renderer();
        let drawn = render(&mut renderer, &composition((8, 4), nodes), frame, &media);
        assert_eq!(drawn.pixel(0, 0), color, "{name}");
        assert_eq!(renderer.recorded_pass_count(), 1, "{name}");
    }
}

#[test]
fn media_keeps_its_native_size_and_a_transform_stores_only_what_it_covers() {
    let composition = composition(
        (1920, 1080),
        json!([
            { "id": 1, "type": "background", "params": {
                "paint": [0, 0, 0, 255], "width": 1920, "height": 1080
            } },
            { "id": 2, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
            { "id": 3, "type": "transform", "in": { "source": 2 } },
            { "id": 4, "type": "merge", "in": { "base": 1, "overlay": 3 } },
        ]),
    );
    let media = TestMedia::default().image("plate", 320, 180, vec![255; 320 * 180 * 4]);
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    render(&mut renderer, &composition, 0.0, &media);
    let domain = |node| {
        renderer
            .recorded_domain(&composition, NodeId::new(node))
            .unwrap()
    };
    assert_eq!(domain(2).storage_size, Size::new(320, 180));
    let transformed = domain(3);
    assert_eq!(transformed.display_size(), Size::new(1920, 1080));
    assert!(
        transformed.data_rect.contains(RectI::new(0, 0, 323, 183))
            && transformed.data_rect.width < 1920
            && transformed.data_rect.height < 1080,
        "{:?}",
        transformed.data_rect
    );
    assert_eq!(domain(4).storage_size, Size::new(1920, 1080));
    assert_eq!(
        renderer.output_desc().unwrap().domain.storage_size,
        Size::new(1920, 1080)
    );
    assert_eq!(renderer.media_texture_upload_count(), 1);
}

#[test]
fn text_at_fractional_position_draws_glyphs_on_whole_pixels() {
    let composition = composition(
        (160, 80),
        json!([text(
            "Sharp text\nwraps across lines",
            json!({
                "font_size": 17, "max_width": 90, "position": [10.3, 20.6],
                "alignment_horizontal": "center", "alignment_vertical": "middle"
            })
        )]),
    );
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let bound = renderer
        .bind_frame(&composition, 0, &TestMedia::default())
        .unwrap();
    let rects: Vec<[f32; 4]> = bound
        .glyph_instances()
        .iter()
        .map(|glyph| glyph.rect)
        .collect();

    assert!(rects.len() > 10);
    assert!(
        rects
            .iter()
            .all(|rect| rect[0].fract() == 0.0 && rect[1].fract() == 0.0),
        "glyph quads must start on whole pixels: {rects:?}"
    );
}

#[test]
fn glyph_scope_crosses_functions() {
    let mut scene = scene(
        (8, 4),
        json!([text("Lumen", json!({ "glyph_offset": "=slide(5)" }))]),
    )
    .frames(12);
    scene.0["functions"] = json!({ "slide(distance)": "vec2(distance * glyph, 0)" });
    let composition: Composition = scene.into();
    gpu()
        .renderer()
        .bind_frame(&composition, 0, &TestMedia::default())
        .unwrap();
}

/// The crop region is clamped to the 8 by 4 source, as origin then size.
#[test]
fn crop_clamps_extreme_values_to_its_source() {
    for (name, crop, expected) in [
        (
            "extents beyond i32",
            [0, 0, 1 << 31, i64::from(u32::MAX)],
            RED,
        ),
        ("origin at i32::MIN", [i64::from(i32::MIN), 0, 4, 4], CLEAR),
        ("origin at i32::MAX", [i64::from(i32::MAX), 2, 4, 4], CLEAR),
    ] {
        let [x, y, width, height] = crop;
        let composition = composition(
            (8, 4),
            json!([
                solid(1, RED),
                { "id": 2, "type": "crop", "in": { "source": 1 },
                  "params": { "x": x, "y": y, "width": width, "height": height } },
            ]),
        );
        let drawn = render(
            &mut gpu().renderer(),
            &composition,
            0.0,
            &TestMedia::default(),
        );
        assert_eq!(drawn.rgba, expected.repeat(32), "{name}");
    }
}

/// A small shape gets a texture around its bounds, rounded out to a size class, not the whole
/// canvas.
#[test]
fn a_small_shape_records_around_its_bounds() {
    let composition = composition(
        (1920, 1080),
        json!([{ "id": 1, "type": "shape", "params": {
            "width": 40, "height": 20, "position": [100, 200],
            "stroke_enabled": true, "stroke_width": 2
        } }]),
    );
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    render(&mut renderer, &composition, 0.0, &TestMedia::default());
    let shape = renderer
        .recorded_domain(&composition, NodeId::new(1))
        .unwrap();
    let data = shape.data_rect;
    assert_eq!(shape.display_size(), Size::new(1920, 1080));
    assert!(data.contains(RectI::new(99, 199, 42, 22)), "{data:?}");
    assert!(data.width <= 48 && data.height <= 32, "{data:?}");
    assert_eq!(shape.storage_size, data.size());
    assert_eq!(
        renderer.output_desc().unwrap().domain.storage_size,
        Size::new(1920, 1080)
    );
}

/// A merge feather that is zero or negative, written as a literal or an expression, records no
/// Gaussian passes.
#[test]
fn only_a_positive_feather_records_passes() {
    let gpu = gpu();
    let media = TestMedia::default();
    let passes = |feather: Value| {
        let composition = composition(
            (8, 4),
            json!([
                { "id": 1, "type": "background", "params": {
                    "paint": [0, 0, 0, 255], "width": 24, "height": 64 } },
                { "id": 2, "type": "background", "params": {
                    "paint": [255, 255, 255, 255], "width": 24, "height": 64 } },
                { "id": 5, "type": "background", "params": {
                    "paint": [255, 255, 255, 255], "width": 24, "height": 64 } },
                { "id": 3, "type": "merge", "params": { "feather": feather },
                  "in": { "base": 1, "overlay": 2, "mask": 5 } },
            ]),
        );
        let mut renderer = gpu.renderer();
        render(&mut renderer, &composition, 0.0, &media);
        renderer.recorded_pass_count()
    };
    let sharp = passes(json!(0.0));
    assert_eq!(passes(json!(-2.0)), sharp);
    assert_eq!(passes(json!("=0")), sharp);
    assert!(passes(json!(2.0)) > sharp);
}

/// A shape under gradient-filled text, whose atlas regions and paint ramp both go through a bind.
fn text_over_shape() -> Composition {
    composition(
        (8, 4),
        json!([
            { "id": 1, "type": "shape", "params": {
                "width": 6, "height": 4, "position": [1, 0], "fill_paint": RED
            } },
            { "id": 2, "type": "text", "params": {
                "content": "hi", "font_size": 4,
                "color": {
                    "type": "linear_gradient",
                    "stops": [
                        { "offset": 0.0, "color": RED },
                        { "offset": 1.0, "color": [0, 0, 255, 255] }
                    ]
                }
            } },
            { "id": 3, "type": "merge", "in": { "base": 1, "overlay": 2 } },
        ]),
    )
}

#[test]
fn text_upload_repeats_until_it_is_uploaded() {
    let composition = text_over_shape();
    let media = TestMedia::default();
    let gpu = gpu();
    let mut renderer = gpu.renderer();

    let first = renderer.bind_frame(&composition, 0, &media).unwrap();
    let regions = first.atlas_region_count();
    assert!(regions > 0);

    let rebound = renderer.bind_frame(&composition, 0, &media).unwrap();
    assert_eq!(
        rebound.atlas_region_count(),
        regions,
        "text uploads that were never applied must be repeated"
    );
    renderer.upload_bound_frame(&rebound).unwrap();

    let unchanged = renderer.bind_frame(&composition, 0, &media).unwrap();
    assert_eq!(unchanged.atlas_region_count(), 0);
    assert!(!unchanged.glyph_instances().is_empty());
}

#[test]
fn text_bind_ahead_restores_the_committed_frame() {
    let composition = composition(
        (8, 4),
        json!([text(
            "Bind ahead",
            json!({ "position": "=vec2(frame * 10, 0)" })
        )]),
    );
    let media = TestMedia::default();
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let first = renderer.bind_frame(&composition, 0, &media).unwrap();
    renderer.upload_bound_frame(&first).unwrap();
    renderer.bind_frame(&composition, 1, &media).unwrap();
    let restored = renderer.bind_frame(&composition, 0, &media).unwrap();
    assert!(!restored.glyph_instances().is_empty());
    assert_eq!(
        restored.atlas_region_count(),
        0,
        "a bind that was never applied must not undo the committed atlas"
    );

    renderer.upload_bound_frame(&restored).unwrap();
    let again = renderer.bind_frame(&composition, 0, &media).unwrap();
    assert_eq!(again.atlas_region_count(), 0);
    let ahead = renderer.bind_frame(&composition, 1, &media).unwrap();
    assert!(!ahead.glyph_instances().is_empty());
}
