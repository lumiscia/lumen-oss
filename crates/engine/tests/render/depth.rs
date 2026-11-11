//! How deep a graph may nest before a frame refuses it, on the 2 MiB stack of a default thread.

use lumen_engine::{
    composition::Composition, diagnostic::Diagnostic, gpu::RenderTarget,
    media::collect_frame_requirements,
};
use serde_json::{Value, json};

use crate::support::{TestMedia, gpu};

/// Runs `run` on a thread with the default 2 MiB stack.
fn on_default_stack<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(run)
        .expect("spawns")
        .join()
        .expect("does not panic")
}

/// A path drawn through `links` nodes of `kind` in a row, into the output, node `links + 2`.
fn chain(kind: &str, params: Value, links: u64) -> Value {
    let mut nodes = vec![json!({ "id": 1, "type": "path", "params": {
        "data": "M 2 2 L 20 2 L 10 16 Z", "fill_paint": "#30dc40"
    } })];
    let mut connections = Vec::new();
    for id in 2..links + 3 {
        let kind = if id == links + 2 {
            "media_output"
        } else {
            kind
        };
        nodes.push(json!({ "id": id, "type": kind, "params": params }));
        connections.push(json!({ "from_node": id - 1, "to_node": id, "to_port": "source" }));
    }
    nodes
        .last_mut()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("params");
    json!({
        "timeline": { "fps": 30, "duration_frames": 1 },
        "render_settings": { "width": 64, "height": 32 },
        "nodes": nodes,
        "connections": connections,
    })
}

/// What rendering `document`'s first frame on a default stack fails with, as a diagnostic, or
/// `None` when it renders. Media prediction runs first, and must agree.
fn first_frame_error(document: Value) -> Option<Diagnostic> {
    on_default_stack(move || {
        let composition: Composition = lumen_engine::json::parse_value(&document).expect("parses");
        let media = TestMedia::default();
        let predicted = collect_frame_requirements(&composition, &media, 0).err();
        let gpu = gpu();
        let rendered = gpu
            .renderer()
            .render(&composition, 0.0, &media, RenderTarget::Owned)
            .err();
        assert_eq!(
            predicted.as_ref().map(ToString::to_string),
            rendered.as_ref().map(ToString::to_string),
            "prediction and rendering disagree"
        );
        rendered.map(|error| Diagnostic::from_error(&error, &document))
    })
}

#[test]
fn a_chain_too_deep_to_record_fails_with_an_error_instead_of_overflowing_the_stack() {
    let error = first_frame_error(chain("transform", json!({ "rotate": 1 }), 500))
        .expect("500 transforms are too deep");
    assert_eq!(error.code, "render.too_deeply_nested");
    // The output, node 502, is the first of the 48 nodes recorded; node 454 is the 49th.
    assert_eq!(error.node.map(|node| node.0), Some(454));
    assert_eq!(error.path, "/nodes/453");
    assert!(
        error
            .message
            .starts_with("node 454 is more than 48 nodes deep"),
        "{}",
        error.message
    );
}

/// Makes the path at the bottom of `chain` evaluate the costliest expression the evaluator allows
/// while every node above it is still recording. The path's `opacity` reads the `rotate` of a
/// `transform`, and each nests right-leaning sums as deep as the 96 evaluation levels allow. The
/// second ends in the measurement of a text node, which shapes text with all those evaluations open.
fn with_the_costliest_expression(mut document: Value) -> Value {
    let nest = |levels: usize, leaf: &str| {
        format!("{}{leaf}{}", "1 + (".repeat(levels), ")".repeat(levels))
    };
    document["nodes"][0]["params"]["opacity"] =
        json!(format!("=min({}, 1)", nest(44, "node(900, 'rotate')")));
    let nodes = document["nodes"].as_array_mut().unwrap();
    nodes.push(json!({ "id": 900, "type": "transform", "params": {
        "rotate": format!("={}", nest(44, "text_width(node(901))"))
    } }));
    nodes.push(
        json!({ "id": 901, "type": "text", "params": { "content": "WWW", "font_size": 16 } }),
    );
    document["connections"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "from_node": 1, "to_node": 900, "to_port": "source" }));
    document
}

#[test]
fn the_deepest_chains_allowed_render_on_a_default_stack_with_the_costliest_expression() {
    // The output, 46 nodes and a path are 48 deep. `transform` has the largest frame of the nodes
    // with an input, with `color_grade` next after keeping its colour table out of it. In a debug
    // build the heaviest chain, 46 transforms, needs about 1.65 MiB of the 2 MiB.
    for (kind, params) in [
        ("color_grade", json!({})),
        ("transform", json!({ "rotate": 1 })),
    ] {
        let document = with_the_costliest_expression(chain(kind, params, 46));
        assert!(first_frame_error(document).is_none(), "{kind}");
    }
}

#[test]
fn a_deep_read_of_an_input_that_is_already_recorded_is_not_too_deep() {
    // The merge records its base, a background, first. The overlay's 46 transforms read that
    // background again as their 49th node, from the cache.
    let mut nodes = vec![
        json!({ "id": 1, "type": "background", "params": { "paint": [200, 30, 30, 255] } }),
        json!({ "id": 2, "type": "merge" }),
        json!({ "id": 3, "type": "media_output" }),
    ];
    let mut connections = vec![
        json!({ "from_node": 1, "to_node": 2, "to_port": "base" }),
        json!({ "from_node": 2, "to_node": 3, "to_port": "source" }),
    ];
    for id in 10..56 {
        nodes.push(json!({ "id": id, "type": "transform", "params": { "rotate": 1 } }));
        let from = if id == 10 { 1 } else { id - 1 };
        connections.push(json!({ "from_node": from, "to_node": id, "to_port": "source" }));
    }
    connections.push(json!({ "from_node": 55, "to_node": 2, "to_port": "overlay" }));
    let document = json!({
        "timeline": { "fps": 30, "duration_frames": 1 },
        "render_settings": { "width": 64, "height": 32 },
        "nodes": nodes,
        "connections": connections,
    });
    assert!(first_frame_error(document).is_none());
}

#[test]
fn templates_nested_through_clips_count_towards_the_depth() {
    // Each template plays the one before it, and its clip reads past the output it plays into:
    // one node per template.
    let mut compositions = serde_json::Map::new();
    for index in 0..60 {
        let mut content = match index {
            0 => json!({ "type": "background", "params": { "paint": [200, 30, 30, 255] } }),
            _ => json!({ "type": "clip", "params": { "composition": format!("t{}", index - 1) } }),
        };
        content["id"] = json!(1);
        compositions.insert(
            format!("t{index}"),
            json!({
                "nodes": [content, { "id": 2, "type": "media_output" }],
                "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }],
            }),
        );
    }
    let document = json!({
        "timeline": { "fps": 30, "duration_frames": 1 },
        "render_settings": { "width": 64, "height": 32 },
        "compositions": compositions,
        "nodes": [
            { "id": 1, "type": "clip", "params": { "composition": "t59" } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }],
    });
    let error = first_frame_error(document).expect("60 nested templates are too deep");
    assert_eq!(error.code, "render.too_deeply_nested");
    // The root's two nodes and the clips of `t59` to `t14` make 48; the 49th is the clip of `t13`.
    assert_eq!(error.path, "/compositions/t13/nodes/0");
    assert!(
        error
            .message
            .starts_with("composition `t13`: node 1 is more than 48 nodes deep"),
        "{}",
        error.message
    );
}
