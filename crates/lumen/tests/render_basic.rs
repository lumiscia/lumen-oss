use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use lumen::{
    AssetCache, Composition, Connection, Graph, InputPort, NodeId, NodeKind, NullMediaStore,
    OutputPort, RasterFrame, RenderContext, RenderSettings, RuntimeCapabilityProfile, SurfacePool,
    TimelineSettings,
    node::{
        Node, ShapeGeometry, blur::Blur, frame_hold::FrameHold, media_output::MediaOutput,
        merge::Merge, shape::Shape, shape_renderer::ShapeRenderer, solid_color::SolidColor,
        switch::Switch, transform::Transform,
    },
};

fn connect(graph: &mut Graph, from: NodeId, to: NodeId, to_port: &str) {
    graph
        .connect(Connection {
            from_node: from,
            from_port: OutputPort::default(),
            to_node: to,
            to_port: InputPort::named(to_port),
        })
        .expect("connection should be valid for test setup")
}

fn render_single(graph: Graph, width: u32, height: u32) -> RasterFrame {
    render_at_frame(graph, width, height, 0)
}

fn render_at_frame(graph: Graph, width: u32, height: u32, frame: u32) -> RasterFrame {
    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 60,
        },
        RenderSettings {
            width,
            height,
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

    composition
        .render_frame(frame, &mut context)
        .expect("render should succeed")
}

fn expect_bitmap(frame: RasterFrame) -> (Arc<Vec<u8>>, u32, u32) {
    match frame {
        RasterFrame::Bitmap(bytes, width, height) => (bytes, width, height),
        RasterFrame::Surface(_) => panic!("expected bitmap output"),
    }
}

#[test]
fn solid_color_to_media_output_renders_expected_bitmap() {
    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [10, 20, 30, 255],
            width: Some(4),
            height: Some(3),
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, solid, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 4, 3));
    assert_eq!((width, height), (4, 3));
    assert_eq!(bytes.len(), 4 * 3 * 4);
    for chunk in bytes.chunks_exact(4) {
        assert_eq!(chunk, &[10, 20, 30, 255]);
    }
}

#[test]
fn transform_translate_shifts_pixels() {
    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(4),
            height: Some(4),
        }),
    ));
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform {
            scale_x: 1.0,
            scale_y: 1.0,
            translate_x: 1.0,
            translate_y: 0.0,
            rotate: 0.0,
            pivot_x: 0.0,
            pivot_y: 0.0,
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, solid, transform, "source");
    connect(&mut graph, transform, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 4, 4));
    assert_eq!((width, height), (4, 4));

    let first_pixel = &bytes[0..4];
    let second_pixel = &bytes[4..8];
    assert_eq!(first_pixel, &[0, 0, 0, 0]);
    assert_eq!(second_pixel, &[255, 0, 0, 255]);
}

#[test]
fn merge_with_half_opacity_blends_base_and_overlay() {
    let mut graph = Graph::new();
    let base = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(2),
            height: Some(2),
        }),
    ));
    let overlay = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [0, 0, 255, 255],
            width: Some(2),
            height: Some(2),
        }),
    ));
    let merge = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Merge(Merge {
            opacity: 0.5,
            ..Merge::default()
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, base, merge, "base");
    connect(&mut graph, overlay, merge, "overlay");
    connect(&mut graph, merge, output, "source");

    let (bytes, _, _) = expect_bitmap(render_single(graph, 2, 2));
    for chunk in bytes.chunks_exact(4) {
        assert!((120..=135).contains(&chunk[0]));
        assert_eq!(chunk[1], 0);
        assert!((120..=135).contains(&chunk[2]));
        assert_eq!(chunk[3], 255);
    }
}

#[test]
fn shape_to_shape_renderer_to_media_output_renders_rectangle() {
    let mut graph = Graph::new();
    let shape = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 3,
                height: 2,
            },
        }),
    ));
    let renderer = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color: [0, 255, 0, 255],
            ..ShapeRenderer::default()
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, shape, renderer, "vector");
    connect(&mut graph, renderer, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 3, 2));
    assert_eq!((width, height), (3, 2));
    for chunk in bytes.chunks_exact(4) {
        assert_eq!(chunk, &[0, 255, 0, 255]);
    }
}

#[test]
fn blur_spreads_single_pixel_into_neighbors() {
    let mut graph = Graph::new();
    let shape = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 3,
                height: 3,
            },
        }),
    ));
    let renderer = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_enabled: false,
            stroke_enabled: true,
            stroke_width: 1.0,
            stroke_color: [255, 0, 0, 255],
            ..ShapeRenderer::default()
        }),
    ));
    let blur = graph.add_node(Node::new(NodeId(0), NodeKind::Blur(Blur { radius: 1.0 })));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, shape, renderer, "vector");
    connect(&mut graph, renderer, blur, "source");
    connect(&mut graph, blur, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 3, 3));
    assert_eq!((width, height), (3, 3));

    let center_alpha = bytes[((1 * 3 + 1) * 4 + 3) as usize];
    let corner_alpha = bytes[3];
    assert!(
        center_alpha > 0,
        "blur should spread border alpha into the transparent center"
    );
    assert!(corner_alpha < 255, "blur should soften edge alpha values");
}

#[test]
fn blur_with_zero_radius_is_passthrough() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [0, 0, 255, 255],
            width: Some(2),
            height: Some(1),
        }),
    ));
    let blur = graph.add_node(Node::new(NodeId(0), NodeKind::Blur(Blur { radius: 0.0 })));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, source, blur, "source");
    connect(&mut graph, blur, output, "source");

    let (bytes, _, _) = expect_bitmap(render_single(graph, 2, 1));
    assert_eq!(bytes.as_slice(), &[0, 0, 255, 255, 0, 0, 255, 255]);
}

#[test]
fn frame_hold_uses_held_frame_for_upstream_evaluation() {
    let mut graph = Graph::new();
    let red = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(1),
            height: Some(1),
        }),
    ));
    let blue = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [0, 0, 255, 255],
            width: Some(1),
            height: Some(1),
        }),
    ));
    let mut switch_map = HashMap::new();
    switch_map.insert(0_u16, 0_u32..10_u32);
    switch_map.insert(1_u16, 10_u32..20_u32);
    let switch = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Switch(Switch::new(switch_map)),
    ));
    let hold = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::FrameHold(FrameHold { hold_frame: 0 }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    graph
        .connect(Connection {
            from_node: red,
            from_port: OutputPort::default(),
            to_node: switch,
            to_port: InputPort::Indexed(0),
        })
        .expect("switch input_0 should connect");
    graph
        .connect(Connection {
            from_node: blue,
            from_port: OutputPort::default(),
            to_node: switch,
            to_port: InputPort::Indexed(1),
        })
        .expect("switch input_1 should connect");
    connect(&mut graph, switch, hold, "source");
    connect(&mut graph, hold, output, "source");

    let (bytes, _, _) = expect_bitmap(render_at_frame(graph, 1, 1, 15));
    assert_eq!(bytes.as_slice(), &[255, 0, 0, 255]);
}

#[test]
fn identity_transform_is_passthrough() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [12, 34, 56, 255],
            width: Some(2),
            height: Some(1),
        }),
    ));
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform::default()),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, source, transform, "source");
    connect(&mut graph, transform, output, "source");

    let (bytes, _, _) = expect_bitmap(render_single(graph, 2, 1));
    assert_eq!(bytes.as_slice(), &[12, 34, 56, 255, 12, 34, 56, 255]);
}
