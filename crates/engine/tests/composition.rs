//! What needs no GPU device: the graphs a composition refuses to build from, and the media it
//! predicts a frame needs.

use lumen_engine::{
    composition::{Composition, CompositionBuilder, RenderSettings, TimelineSettings},
    error::{GraphValidationError, LumenError},
    graph::{Connection, Graph},
    media::{
        CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver,
        VideoMetadata, collect_frame_requirements,
    },
    node::{
        Deferred, Node, NodeId, NodeKind,
        compositing::{merge::Merge, raster_multimerge::RasterMultiMerge},
        media_output::MediaOutput,
        processing::{blur::Blur, exposure::Exposure, opacity::Opacity, time_remap::TimeRemap},
        source::background::Background,
        source::media_in::{MediaIn, MediaInParamsDelegate},
    },
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// A one-frame scene of `nodes` on a `width` by `height` canvas. A node's `in` maps each input
/// port to a node id, or to an array of them for a variadic port, and the last node feeds the
/// output.
fn scene((width, height): (u32, u32), nodes: Value) -> Composition {
    let Value::Array(mut nodes) = nodes else {
        panic!("scene nodes must be an array");
    };
    let mut connections = Vec::new();
    for node in &mut nodes {
        let id = node["id"].clone();
        let Some(inputs) = node.as_object_mut().and_then(|node| node.remove("in")) else {
            continue;
        };
        for (port, from) in inputs.as_object().expect("`in` must map ports to node ids") {
            let sources = from
                .as_array()
                .map_or(std::slice::from_ref(from), Vec::as_slice);
            for source in sources {
                connections.push(json!({ "from_node": source, "to_node": id, "to_port": port }));
            }
        }
    }
    let last = nodes.last().expect("scene needs a node")["id"].clone();
    nodes.push(json!({ "id": 9999, "type": "media_output" }));
    connections.push(json!({ "from_node": last, "to_node": 9999, "to_port": "source" }));
    lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 24, "duration_frames": 12 },
        "render_settings": { "width": width, "height": height },
        "nodes": nodes,
        "connections": connections,
    }))
    .unwrap_or_else(|error| panic!("scene does not parse: {error:#}"))
}

fn solid(id: u64, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "background", "params": {
        "paint": color, "width": 8, "height": 4
    } })
}

/// Uniform buffers bound by the passes compiled for `node`, whose labels carry its id.
#[derive(Debug, Clone)]
struct TestMediaStore {
    id: String,
    frame: Arc<CpuMediaFrame>,
}

impl TestMediaStore {
    fn image(id: &str, width: u32, height: u32) -> Self {
        Self {
            id: id.to_string(),
            frame: Arc::new(CpuMediaFrame {
                rgba: Arc::new(vec![255; width as usize * height as usize * 4]),
                width,
                height,
                row_bytes: width as usize * 4,
            }),
        }
    }
}

impl MediaStore for TestMediaStore {
    fn get_image_resolver(
        &self,
        source: &str,
    ) -> Result<Box<dyn ImageResolver>, lumen_engine::error::MediaError> {
        (source == self.id)
            .then(|| Box::new(self.clone()) as Box<dyn ImageResolver>)
            .ok_or_else(|| {
                lumen_engine::error::MediaError::source_not_found(source, "not the test image")
            })
    }
}

impl ImageResolver for TestMediaStore {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> ImageMetadata {
        ImageMetadata {
            width: self.frame.width,
            height: self.frame.height,
        }
    }

    fn frame(&self) -> Result<MediaFrame, lumen_engine::error::MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.frame)))
    }
}

fn build(graph: Graph) -> Result<Composition, Vec<LumenError>> {
    CompositionBuilder::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 1,
        },
        RenderSettings {
            width: 100,
            height: 100,
        },
    )
    .build()
}

fn build_errors(graph: Graph) -> Vec<LumenError> {
    build(graph).expect_err("invalid graph must be rejected before compilation")
}

fn assert_cycle_rejected_at_build(graph: Graph) {
    assert!(matches!(
        build_errors(graph).as_slice(),
        [LumenError::GraphValidation(
            GraphValidationError::Cycle { .. }
        )]
    ));
}

#[test]
fn a_cycle_error_prints_the_cycle_and_not_the_nodes_it_feeds() {
    let blur = |id, source| {
        NodeKind::Blur(Blur {
            id: NodeId::new(id),
            params: Default::default(),
            source: Some(NodeId::new(source)),
        })
    };
    let mut graph = Graph::new();
    for node in [blur(1, 3), blur(3, 1), blur(4, 3)] {
        graph.nodes.insert(node.id(), node);
    }
    let errors = build_errors(graph);
    assert!(
        errors
            .iter()
            .any(|error| error.to_string() == "graph contains a cycle: 1 -> 3 -> 1"),
        "{errors:?}"
    );
}

#[test]
fn a_missing_input_node_is_not_reported_as_a_cycle() {
    let mut graph = Graph::new();
    let output = NodeId::new(1);
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: Some(NodeId::new(999)),
        }),
    );
    assert!(matches!(
        build_errors(graph).as_slice(),
        [LumenError::GraphValidation(
            GraphValidationError::MissingSourceNode { node_id }
        )] if *node_id == NodeId::new(999)
    ));
}

#[test]
fn cycles_are_rejected_at_build() {
    let first = NodeId::new(1);
    let second = NodeId::new(2);
    let output = NodeId::new(3);
    let connection = |from, to| Connection {
        from_node: from,
        to_node: to,
        to_port: "source".to_string(),
    };
    let exposure = |id, source| {
        NodeKind::Exposure(Exposure {
            id,
            source: Some(source),
            ..Exposure::default()
        })
    };
    let media_output = || {
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: Some(first),
        })
    };

    // Connections written into the inputs.
    let mut graph = Graph::new();
    for id in [first, second] {
        graph.nodes.insert(
            id,
            NodeKind::Exposure(Exposure {
                id,
                ..Exposure::default()
            }),
        );
    }
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            ..MediaOutput::default()
        }),
    );
    for edge in [
        connection(first, second),
        connection(second, first),
        connection(first, output),
    ] {
        graph.connect(edge).unwrap();
    }
    assert_cycle_rejected_at_build(graph);

    // Nodes built directly carry their inputs.
    let mut graph = Graph::new();
    graph.nodes.insert(first, exposure(first, second));
    graph.nodes.insert(second, exposure(second, first));
    graph.nodes.insert(output, media_output());
    assert_cycle_rejected_at_build(graph);

    let mut graph = Graph::new();
    graph.nodes.insert(
        first,
        NodeKind::TimeRemap(TimeRemap {
            id: first,
            source: Some(first),
            ..TimeRemap::default()
        }),
    );
    graph.nodes.insert(output, media_output());
    assert_cycle_rejected_at_build(graph);

    // A node stored under another node's id would hide a cycle from the edges.
    let mut graph = Graph::new();
    graph.nodes.insert(
        first,
        NodeKind::TimeRemap(TimeRemap {
            id: second,
            source: Some(first),
            ..TimeRemap::default()
        }),
    );
    graph.nodes.insert(output, media_output());
    assert!(build_errors(graph).iter().any(|error| matches!(
        error,
        LumenError::GraphValidation(GraphValidationError::NodeIdMismatch { .. })
    )));
}

#[test]
fn a_missing_required_input_is_named() {
    let output = NodeId::new(1);
    let mut graph = Graph::new();
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            ..MediaOutput::default()
        }),
    );
    let errors = build_errors(graph);
    assert!(
        errors
            .iter()
            .any(|error| error.to_string() == "node 1 is missing required input `source`"),
        "{errors:?}"
    );
}

#[test]
fn a_second_connection_to_a_single_input_is_rejected_and_a_variadic_input_appends() {
    let (first, second, merge) = (NodeId::new(1), NodeId::new(2), NodeId::new(3));
    let connect = |graph: &mut Graph, from_node, to_port: &str| {
        graph.connect(Connection {
            from_node,
            to_node: merge,
            to_port: to_port.to_string(),
        })
    };
    let mut graph = Graph::new();
    for id in [first, second] {
        graph.nodes.insert(
            id,
            NodeKind::Background(Background {
                id,
                ..Background::default()
            }),
        );
    }
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            ..Merge::default()
        }),
    );
    connect(&mut graph, first, "base").unwrap();
    let error = connect(&mut graph, second, "base").unwrap_err();
    assert_eq!(
        error.to_string(),
        "input `base` of node 3 is already connected"
    );

    let multi = NodeId::new(4);
    graph.nodes.insert(
        multi,
        NodeKind::RasterMultiMerge(RasterMultiMerge {
            id: multi,
            ..RasterMultiMerge::default()
        }),
    );
    for from_node in [first, second] {
        graph
            .connect(Connection {
                from_node,
                to_node: multi,
                to_port: "layers".to_string(),
            })
            .unwrap();
    }
}

#[test]
fn media_prediction_follows_direct_ports() {
    let (media, opacity, output) = (NodeId::new(1), NodeId::new(2), NodeId::new(3));
    let mut graph = Graph::new();
    graph.nodes.insert(
        media,
        NodeKind::MediaIn(MediaIn {
            id: media,
            params: MediaInParamsDelegate {
                source: Deferred::value("plate".to_string()),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        opacity,
        NodeKind::Opacity(Opacity {
            id: opacity,
            source: Some(NodeId::new(1)),
            ..Opacity::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: Some(NodeId::new(2)),
        }),
    );
    let composition = build(graph).unwrap_or_else(|errors| panic!("{errors:?}"));

    let requirements =
        collect_frame_requirements(&composition, &TestMediaStore::image("plate", 8, 4), 0).unwrap();

    assert_eq!(requirements.images, vec!["plate"]);
    assert!(requirements.videos.is_empty());
}

#[test]
fn media_prediction_visits_shared_inputs_once() {
    #[derive(Debug, Default)]
    struct CountingStore(AtomicUsize);
    impl MediaStore for CountingStore {
        fn get_video_resolver(
            &self,
            _: &str,
        ) -> Result<Box<dyn VideoFrameResolver>, lumen_engine::error::MediaError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(OneFrameVideo))
        }
    }
    struct OneFrameVideo;
    impl VideoFrameResolver for OneFrameVideo {
        fn id(&self) -> &str {
            "clip"
        }
        fn metadata(&self) -> VideoMetadata {
            VideoMetadata {
                width: 8,
                height: 4,
                frame_count: 1,
                fps: 24.0,
            }
        }
        fn frame(&self, _: u32) -> Result<MediaFrame, lumen_engine::error::MediaError> {
            unreachable!("prediction reads no frames")
        }
    }

    // Each merge reads the one below it twice, so a walk without memory reaches the clip 16 times.
    let mut nodes = vec![
        json!({ "id": 1, "type": "media_in", "params": { "kind": "video", "source": "clip" } }),
    ];
    for level in 1..=4u64 {
        nodes.push(json!({
            "id": level + 1, "type": "merge", "in": { "base": level, "overlay": level }
        }));
    }
    let store = CountingStore::default();

    collect_frame_requirements(&scene((8, 4), json!(nodes)), &store, 0).unwrap();

    assert_eq!(store.0.load(Ordering::Relaxed), 1);
}

#[test]
fn media_prediction_follows_a_videos_range_and_loop_mode() {
    #[derive(Debug)]
    struct TenFrameStore;
    impl MediaStore for TenFrameStore {
        fn get_video_resolver(
            &self,
            _: &str,
        ) -> Result<Box<dyn VideoFrameResolver>, lumen_engine::error::MediaError> {
            Ok(Box::new(TenFrameVideo))
        }
    }
    struct TenFrameVideo;
    impl VideoFrameResolver for TenFrameVideo {
        fn id(&self) -> &str {
            "clip"
        }
        fn metadata(&self) -> VideoMetadata {
            VideoMetadata {
                width: 8,
                height: 4,
                frame_count: 10,
                fps: 24.0,
            }
        }
        fn frame(&self, _: u32) -> Result<MediaFrame, lumen_engine::error::MediaError> {
            unreachable!("prediction reads no frames")
        }
    }

    let predicted = |params: Value, frame: u32| {
        let mut params = params.as_object().unwrap().clone();
        params.insert("kind".into(), json!("video"));
        params.insert("source".into(), json!("clip"));
        let composition = scene(
            (8, 4),
            json!([{ "id": 1, "type": "media_in", "params": params }]),
        );
        collect_frame_requirements(&composition, &TenFrameStore, frame).map(|requirements| {
            requirements
                .videos
                .into_iter()
                .flat_map(|video| video.frames)
                .collect::<Vec<_>>()
        })
    };

    // The end is the first frame past the range, and a start alone plays to the source's end.
    for (params, frame, expected) in [
        (
            json!({ "range_start": 4, "range_end": 7, "loop_mode": "repeat" }),
            3,
            4,
        ),
        (json!({ "range_start": 4, "range_end": 7 }), 9, 6),
        (json!({ "range_start": 4 }), 9, 9),
        (json!({ "range_end": 3, "loop_mode": "ping_pong" }), 3, 1),
        (json!({ "loop_mode": "=1" }), 12, 2),
    ] {
        assert_eq!(
            predicted(params.clone(), frame).unwrap(),
            [expected],
            "{params} at {frame}"
        );
    }

    let error = predicted(json!({ "range_start": 6, "range_end": 6 }), 0).unwrap_err();
    assert!(
        error.to_string().contains("out of range for clip"),
        "{error}"
    );
    let error = predicted(json!({ "loop_mode": "=7" }), 0).unwrap_err();
    assert!(
        error.to_string().contains("7 is not a value of LoopMode"),
        "{error}"
    );
}

#[test]
fn color_grade_interpolation_is_a_sampling_name() {
    let load = |value: Value| {
        lumen_engine::json::parse_value(&json!({
            "timeline": { "fps": 24, "duration_frames": 12 },
            "render_settings": { "width": 8, "height": 4 },
            "nodes": [
                solid(1, [255, 0, 0, 255]),
                { "id": 2, "type": "color_grade", "params": { "interpolation": value } },
                { "id": 3, "type": "media_output" },
            ],
            "connections": [
                { "from_node": 1, "to_node": 2, "to_port": "source" },
                { "from_node": 2, "to_node": 3, "to_port": "source" },
            ],
        }))
    };
    load(json!("nearest")).unwrap();
    for value in [json!(0), json!("cubic")] {
        assert!(load(value.clone()).is_err(), "{value}");
    }
}
