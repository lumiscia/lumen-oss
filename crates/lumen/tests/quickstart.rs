use std::sync::{Arc, RwLock};

use lumen::{
    AssetCache, Composition, Connection, Graph, InputPort, NodeId, NodeKind, NullMediaStore,
    OutputPort, RasterFrame, RenderContext, RenderSettings, RuntimeCapabilityProfile, SurfacePool,
    TimelineSettings,
    node::{Node, media_output::MediaOutput, solid_color::SolidColor},
};

#[test]
fn quickstart_example_compiles_and_renders() {
    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(16),
            height: Some(16),
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    graph
        .connect(Connection {
            from_node: solid,
            from_port: OutputPort::default(),
            to_node: output,
            to_port: InputPort::named("source"),
        })
        .expect("quickstart graph connection should be valid");

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 1,
        },
        RenderSettings {
            width: 16,
            height: 16,
            background_color: [0, 0, 0, 0],
        },
    );

    composition
        .validate(&RuntimeCapabilityProfile::cpu_only())
        .expect("quickstart composition should validate");

    let mut ctx = RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        Arc::new(NullMediaStore),
        RuntimeCapabilityProfile::cpu_only(),
    );

    let frame = composition
        .render_frame(0, &mut ctx)
        .expect("quickstart render should succeed");
    let bytes = match frame {
        RasterFrame::Bitmap(bytes, ..) => bytes,
        RasterFrame::Surface(_) => Arc::new(Vec::new()),
    };
    assert_eq!(bytes.len(), 16 * 16 * 4);
}
