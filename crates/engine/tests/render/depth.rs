//! How deep a graph may nest before a frame refuses it, on the 2 MiB stack of a default thread.

use lumen_engine::{
    composition::Composition, diagnostic::Diagnostic, media::collect_frame_requirements,
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
            .render_frame_at(&composition, 0.0, &media)
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

#[test]
fn the_deepest_chain_allowed_renders_on_a_default_stack() {
    // `color_grade` records with the largest stack frame of any node with an input, and paths with
    // the largest of the sources: the output, 46 of them and a path are 48 nodes deep.
    assert!(first_frame_error(chain("color_grade", json!({}), 46)).is_none());
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
