//! The per-node bind time a render reports when it profiles nodes.

use std::time::{Duration, Instant};

use lumen_engine::{composition::Composition, gpu::RenderTarget};

use crate::clips::probe_scene;
use crate::support::{TestMedia, gpu};
use serde_json::json;

/// A root clip of a `probe` template. Both have a node 1 and a node 2. A clip evaluates what its
/// template's output shows, so the template's `media_output` is not a node it records.
fn composition() -> Composition {
    probe_scene(10.0, json!({ "composition": "probe", "start": 0 })).into()
}

#[test]
fn profiling_reports_each_node_by_template_and_less_than_the_bind() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_profile_nodes(true);
    let composition = composition();
    let media = TestMedia::default();
    let mut bind = Duration::ZERO;
    for frame in 0..3 {
        let started = Instant::now();
        renderer
            .bind_frame(&composition, f64::from(frame), &media, RenderTarget::Owned)
            .expect("frame binds");
        bind += started.elapsed();
    }

    let timing = renderer.take_node_timing();
    let mut nodes: Vec<_> = timing
        .iter()
        .map(|node| {
            (
                node.template.as_deref(),
                node.node.0,
                node.kind,
                node.frames,
            )
        })
        .collect();
    nodes.sort();
    assert_eq!(
        nodes,
        [
            (None, 1, "clip", 3),
            (None, 2, "media_output", 3),
            (Some("probe"), 1, "background", 3),
        ]
    );
    assert!(timing.windows(2).all(|pair| pair[0].bind >= pair[1].bind));
    let own: Duration = timing.iter().map(|node| node.bind).sum();
    assert!(own <= bind, "{own:?} of nodes in a bind of {bind:?}");
    assert!(renderer.take_node_timing().is_empty());
}

#[test]
fn nodes_are_not_timed_unless_profiling_is_on() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let composition = composition();
    renderer
        .bind_frame(
            &composition,
            0.0,
            &TestMedia::default(),
            RenderTarget::Owned,
        )
        .expect("frame binds");
    assert!(renderer.take_node_timing().is_empty());
}
