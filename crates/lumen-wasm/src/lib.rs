use std::ffi::c_char;

use lumen::{
	node::{media_output::MediaOutput, solid_color::SolidColor, Node},
	Composition, Connection, Graph, InputPort, NodeId, NodeKind, OutputPort, RenderSettings,
	TimelineSettings,
};

static VERSION: &[u8] = b"lumen-wasm-next\0";

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_version() -> *const c_char {
	VERSION.as_ptr() as *const c_char
}

#[unsafe(no_mangle)]
pub extern "C" fn lumen_wasm_build_demo_composition() -> usize {
	let mut graph = Graph::new();
	let solid = graph.add_node(Node::new(
		NodeId(0),
		NodeKind::SolidColor(SolidColor {
			color: [0, 255, 0, 255],
			width: Some(32),
			height: Some(32),
		}),
	));
	let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
	if graph
		.connect(Connection {
			from_node: solid,
			from_port: OutputPort::default(),
			to_node: output,
			to_port: InputPort::named("source"),
		})
		.is_err()
	{
		return 0;
	}

	let composition = Composition::new(
		graph,
		TimelineSettings {
			fps: 30.0,
			duration_frames: 1,
		},
		RenderSettings {
			width: 32,
			height: 32,
			background_color: [0, 0, 0, 0],
		},
	);

	composition.graph.nodes.len()
}
