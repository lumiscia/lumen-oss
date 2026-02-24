use std::collections::HashMap;

use lumen::{
    Connection, InputPort, LumenError, NodeId, NodeKind, OutputPort,
    error::GraphValidationError,
    graph::Graph,
    node::{
        Node, ShapeGeometry, crop::Crop, media_output::MediaOutput, shape::Shape,
        solid_color::SolidColor, switch::Switch, transform::Transform,
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

#[test]
fn cycle_detection_rejects_cyclic_graph() {
    let mut graph = Graph::new();
    let a = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform::default()),
    ));
    let b = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Crop(Crop {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, a, b, "source");
    connect(&mut graph, b, a, "source");
    connect(&mut graph, a, output, "source");

    let errors = graph.validate().expect_err("graph should be cyclic");
    assert!(errors.iter().any(|error| {
        matches!(
            error,
            LumenError::GraphValidation(GraphValidationError::Cycle { path }) if !path.is_empty()
        )
    }));
}

#[test]
fn missing_required_input_rejects_graph() {
    let mut graph = Graph::new();
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform::default()),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, transform, output, "source");

    let errors = graph
        .validate()
        .expect_err("transform source input is required");
    assert!(errors.iter().any(|error| {
		matches!(
			error,
			LumenError::GraphValidation(GraphValidationError::MissingRequiredInput { node_id, node_kind, .. })
				if *node_id == transform && *node_kind == "Transform"
		)
	}));
}

#[test]
fn port_type_mismatch_rejects_graph() {
    let mut graph = Graph::new();
    let shape = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 4,
                height: 4,
            },
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, shape, output, "source");

    let errors = graph
        .validate()
        .expect_err("vector to raster connection should fail");
    assert!(errors.iter().any(|error| {
		matches!(
			error,
			LumenError::GraphValidation(GraphValidationError::PortKindMismatch { from_node, to_node, .. })
				if *from_node == shape && *to_node == output
		)
	}));
}

#[test]
fn valid_solid_transform_media_output_graph_passes_validation() {
    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor::default()),
    ));
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform::default()),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, solid, transform, "source");
    connect(&mut graph, transform, output, "source");

    assert!(graph.validate().is_ok(), "expected valid graph");
}

#[test]
fn overlapping_switch_ranges_are_rejected() {
    let mut map = HashMap::new();
    map.insert(0_u16, 0_u32..10_u32);
    map.insert(1_u16, 5_u32..15_u32);

    let mut graph = Graph::new();
    let switch = graph.add_node(Node::new(NodeId(0), NodeKind::Switch(Switch::new(map))));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, switch, output, "source");

    let errors = graph
        .validate()
        .expect_err("switch overlap must fail validation");
    assert!(errors.iter().any(|error| {
        matches!(
            error,
            LumenError::GraphValidation(GraphValidationError::SwitchRangeOverlap { node_id, .. })
                if *node_id == switch
        )
    }));
}
