use std::sync::{Arc, RwLock};

use lumen::{
	node::{
		media_output::MediaOutput, merge::Merge, shape::Shape, shape_renderer::ShapeRenderer,
		solid_color::SolidColor, transform::Transform, Node, ShapeGeometry,
	},
	AssetCache, Composition, Connection, Graph, InputPort, NodeId, NodeKind, NullMediaStore,
	OutputPort, RasterFrame, RenderContext, RenderSettings, RuntimeCapabilityProfile,
	SurfacePool, TimelineSettings,
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
		.render_frame(0, &mut context)
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
			scale: (1.0, 1.0),
			translate: (1.0, 0.0),
			rotate_degrees: 0.0,
			pivot: (0.0, 0.0),
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
			color: [0, 255, 0, 255],
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
