use std::sync::{Arc, RwLock};

use anyhow::Result;
use lumen::{
    AssetCache, Composition, Connection, Graph, InputPort, NodeId, NodeKind, NullMediaStore,
    OutputPort, RasterFrame, RenderContext, RenderSettings, RuntimeCapabilityProfile, SurfacePool,
    TimelineSettings,
    node::{Node, media_output::MediaOutput, solid_color::SolidColor},
};

fn main() -> Result<()> {
    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(64),
            height: Some(64),
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    graph.connect(Connection {
        from_node: solid,
        from_port: OutputPort::default(),
        to_node: output,
        to_port: InputPort::named("source"),
    })?;

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 1,
        },
        RenderSettings {
            width: 64,
            height: 64,
            background_color: [0, 0, 0, 0],
        },
    );

    let mut context = RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        Arc::new(NullMediaStore),
        RuntimeCapabilityProfile::cpu_only(),
    );

    let frame = composition.render_frame(0, &mut context)?;
    let bytes = match frame {
        RasterFrame::Bitmap(bitmap) => bitmap.pixels,
        RasterFrame::Surface(_) => Arc::new(Vec::new()),
    };

    println!("rendered {} bytes", bytes.len());
    Ok(())
}
