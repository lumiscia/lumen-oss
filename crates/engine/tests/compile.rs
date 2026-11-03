//! Compile and bind contracts that need no GPU device: what a composition compiles to, and what
//! each frame bind uploads or rejects.

use lumen_engine::{
    composition::{Composition, CompositionBuilder, RenderSettings, TimelineSettings},
    error::{GraphValidationError, LumenError},
    expr::Expression,
    gpu::{AlphaMode, BoundFrame, CompileContext, CompiledComposition, FrameBindContext},
    graph::{Connection, Graph},
    media::{
        CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver,
        VideoMetadata, collect_frame_requirements,
    },
    node::{
        Deferred, NodeId, NodeKind, PortRef,
        media_output::MediaOutput,
        processing::{
            alpha_premultiply::{AlphaPremultiply, AlphaPremultiplyParamsDelegate},
            exposure::Exposure,
            opacity::Opacity,
            time_remap::TimeRemap,
        },
        source::background::Background,
        source::media_in::{MediaIn, MediaInParamsDelegate},
        source::text::{Text, TextParamsDelegate},
        source::text_content::TextContentDelegate,
        vector::{
            paint::{GradientPaint, GradientStop, Paint, PaintKind},
            shape::{Shape, ShapeParamsDelegate},
        },
    },
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const RGBA8: lumen_gpu::wgpu::TextureFormat = lumen_gpu::wgpu::TextureFormat::Rgba8Unorm;

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

fn try_compile(
    composition: &Composition,
    frame: Option<u32>,
) -> Result<CompiledComposition, LumenError> {
    match frame {
        Some(frame) => CompileContext::with_frame(composition, frame, RGBA8),
        None => CompileContext::new(composition),
    }
    .compile()
}

fn compile(composition: &Composition, frame: Option<u32>) -> CompiledComposition {
    try_compile(composition, frame).unwrap()
}

fn port(node: u64) -> PortRef {
    PortRef::new(NodeId::new(node), "output".to_string())
}

fn has_output(compiled: &CompiledComposition, node: u64) -> bool {
    compiled.node_outputs.contains_key(&port(node))
}

fn raster(compiled: &CompiledComposition, node: u64) -> lumen_engine::gpu::RasterHandle {
    compiled.node_outputs[&port(node)]
        .clone()
        .into_raster(NodeId::new(node), "output")
        .unwrap()
}

/// Uniform buffers bound by the passes compiled for `node`, whose labels carry its id.
fn uniform_buffers_of(plan: &lumen_gpu::RenderPlan, node: u64) -> Vec<lumen_gpu::BufferId> {
    let marker = format!(":{node}:");
    plan.passes()
        .iter()
        .filter_map(|pass| match &pass.desc {
            lumen_gpu::PassDesc::Compute(desc)
                if desc
                    .label
                    .as_deref()
                    .is_some_and(|label| label.contains(&marker)) =>
            {
                Some(desc)
            }
            _ => None,
        })
        .flat_map(|desc| &desc.bindings)
        .filter_map(|binding| match binding.resource {
            lumen_gpu::BindingResource::Buffer {
                id,
                access: lumen_gpu::BufferAccess::Uniform,
            } => Some(id),
            _ => None,
        })
        .collect()
}

fn uploaded(bound: &BoundFrame, buffer: lumen_gpu::BufferId) -> &[u8] {
    bound
        .frame_update()
        .uploads()
        .iter()
        .find_map(|upload| match upload {
            lumen_gpu::Upload::Buffer { id, data, .. } if *id == buffer => Some(*data),
            _ => None,
        })
        .expect("the buffer is bound")
}

fn uploaded_f32s(bound: &BoundFrame, buffer: lumen_gpu::BufferId, count: usize) -> Vec<f32> {
    uploaded(bound, buffer)[..count * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_ne_bytes(*bytes))
        .collect()
}

fn bind(composition: &Composition, compiled: &CompiledComposition, frame: u32) -> BoundFrame {
    FrameBindContext::new(composition, frame)
        .bind(compiled)
        .unwrap()
}

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
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        (source == self.id).then(|| Box::new(self.clone()) as Box<dyn ImageResolver>)
    }

    fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
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
            background_color: [0, 0, 0, 255],
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
fn a_missing_input_node_is_not_reported_as_a_cycle() {
    let mut graph = Graph::new();
    let output = NodeId::new(1);
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(NodeId::new(999), "output".to_string()),
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
    let port = |id| PortRef::new(id, "output".to_string());
    let connection = |from, to| Connection {
        from_node: from,
        from_port: "output".to_string(),
        to_node: to,
        to_port: "source".to_string(),
    };
    let exposure = |id, source| {
        NodeKind::Exposure(Exposure {
            id,
            source: port(source),
            ..Exposure::default()
        })
    };
    let media_output = || {
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: port(first),
        })
    };

    // Ports and connections agree.
    let mut graph = Graph::new();
    graph.nodes.insert(first, exposure(first, second));
    graph.nodes.insert(second, exposure(second, first));
    graph.nodes.insert(output, media_output());
    for edge in [
        connection(first, second),
        connection(second, first),
        connection(first, output),
    ] {
        graph.connect(edge).unwrap();
    }
    assert_cycle_rejected_at_build(graph);

    // Nodes built directly carry their inputs on ports, with no connections at all.
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
            source: port(first),
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
            source: port(first),
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
fn animated_exposure_binds_its_uniform() {
    let composition = lumen_engine::json::parse(
        r#"{
            "timeline": { "fps": 24, "duration_frames": 12 },
            "render_settings": { "width": 8, "height": 4 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [64, 128, 255, 255], "width": 8, "height": 4 } },
                { "id": 2, "type": "exposure", "params": { "exposure": "=frame / 10", "contrast": 1.5, "offset": 0.25 } },
                { "id": 3, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" },
                { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "source" }
            ]
        }"#,
    )
    .unwrap();
    let compiled = compile(&composition, None);
    let [buffer] = uniform_buffers_of(&compiled.plan, 2)[..] else {
        panic!("exposure binds one uniform buffer");
    };

    // The uniform holds the exposure, contrast and offset, in that order.
    for (frame, exposure) in [(0, 0.0), (10, 1.0)] {
        let bound = bind(&composition, &compiled, frame);
        assert_eq!(
            uploaded_f32s(&bound, buffer, 3),
            [exposure, 1.5, 0.25],
            "frame {frame}"
        );
    }
}

#[test]
fn media_input_domain() {
    let plate =
        json!({ "id": 2, "type": "media_in", "params": { "kind": "image", "source": "plate" } });

    // Without metadata the plate is a canvas-sized frame texture, copied into a writable output.
    let composition = scene(
        (16, 9),
        json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } }]),
    );
    let compiled = compile(&composition, None);
    assert_eq!(compiled.plan.textures().len(), 2);
    assert_eq!(compiled.plan.buffers().len(), 0);
    assert_eq!(compiled.plan.passes().len(), 1);
    assert_eq!(
        compiled.output.domain.storage_size,
        lumen_gpu::Size::new(16, 9)
    );

    // With metadata it keeps its native size, and a transform stores only the pixels it covers.
    let composition = scene(
        (1920, 1080),
        json!([
            { "id": 1, "type": "background", "params": {
                "paint": [0, 0, 0, 255], "width": 1920, "height": 1080
            } },
            plate,
            { "id": 3, "type": "transform", "in": { "source": 2 } },
            { "id": 4, "type": "merge", "in": { "base": 1, "overlay": 3 } },
        ]),
    );
    let store = TestMediaStore::image("plate", 320, 180);
    let compiled = CompileContext::with_media(&composition, &store, RGBA8)
        .compile()
        .unwrap();
    assert_eq!(
        raster(&compiled, 2).domain.storage_size,
        lumen_gpu::Size::new(320, 180)
    );
    let transformed = raster(&compiled, 3).domain;
    assert_eq!(transformed.display_size(), lumen_gpu::Size::new(1920, 1080));
    assert_eq!(transformed.data_rect, lumen_gpu::RectI::new(0, 0, 323, 183));
    assert_eq!(
        raster(&compiled, 4).domain.storage_size,
        lumen_gpu::Size::new(1920, 1080)
    );
    assert_eq!(
        compiled.output.domain.storage_size,
        lumen_gpu::Size::new(1920, 1080)
    );

    let bound = FrameBindContext::with_media(&composition, 0, &store)
        .bind(&compiled)
        .unwrap();
    assert_eq!(bound.media_textures().len(), 1);
    assert_eq!(
        bound.media_textures()[0].size,
        lumen_gpu::Size::new(320, 180)
    );
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
                kind: Deferred::value(0),
                source: Deferred::value("plate".to_string()),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        opacity,
        NodeKind::Opacity(Opacity {
            id: opacity,
            source: port(1),
            ..Opacity::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: port(2),
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
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }
        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Some(Box::new(OneFrameVideo))
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
fn routing_selects_one_branch() {
    let layers = |selected: Value| {
        json!([
            solid(1, [255, 0, 0, 255]),
            solid(2, [0, 255, 0, 255]),
            { "id": 3, "type": "switch", "params": { "selected_layer": selected },
              "in": { "layers": [1, 2] } },
        ])
    };
    let frame_switch = layers(json!("=if(frame < 6, 0, 1)"));
    // `aliases` must share the texture of `selected`, and `unselected` must not be compiled.
    struct Row {
        name: &'static str,
        nodes: Value,
        frame: Option<u32>,
        aliases: &'static [u64],
        selected: u64,
        unselected: Option<u64>,
    }
    let rows = [
        Row {
            name: "literal switch",
            nodes: layers(json!(0)),
            frame: None,
            aliases: &[3],
            selected: 1,
            unselected: Some(2),
        },
        Row {
            name: "frame switch, first branch",
            nodes: frame_switch.clone(),
            frame: Some(0),
            aliases: &[3],
            selected: 1,
            unselected: Some(2),
        },
        Row {
            name: "frame switch, second branch",
            nodes: frame_switch.clone(),
            frame: Some(10),
            aliases: &[3],
            selected: 2,
            unselected: Some(1),
        },
        Row {
            name: "time remap and memo",
            nodes: json!([
                solid(1, [64, 128, 255, 255]),
                { "id": 2, "type": "time_remap", "params": {
                    "frame": 8.0, "loop_enabled": true, "loop_start": 4, "loop_end": 12
                }, "in": { "source": 1 } },
                { "id": 3, "type": "memo", "in": { "source": 2 } },
            ]),
            frame: None,
            aliases: &[2, 3],
            selected: 1,
            unselected: None,
        },
        Row {
            // The switch is evaluated at the remapped frame 0, not at the compile frame 10.
            name: "remap compiles its source at the remapped frame",
            nodes: json!([
                solid(1, [255, 0, 0, 255]),
                solid(2, [0, 255, 0, 255]),
                { "id": 3, "type": "switch", "params": { "selected_layer": "=if(frame < 6, 0, 1)" },
                  "in": { "layers": [1, 2] } },
                { "id": 4, "type": "time_remap", "params": {
                    "frame": 0.0, "loop_enabled": false, "loop_start": 0, "loop_end": 0
                }, "in": { "source": 3 } },
            ]),
            frame: Some(10),
            aliases: &[4],
            selected: 1,
            unselected: Some(2),
        },
    ];
    for row in rows {
        let composition = scene((8, 4), row.nodes);
        let compiled = try_compile(&composition, row.frame)
            .unwrap_or_else(|error| panic!("{}: {error}", row.name));
        let selected = raster(&compiled, row.selected);
        for alias in row.aliases {
            assert_eq!(
                raster(&compiled, *alias).texture,
                selected.texture,
                "{}",
                row.name
            );
        }
        if let Some(unselected) = row.unselected {
            assert!(!has_output(&compiled, unselected), "{}", row.name);
        }
        // Only the selected solid is drawn, so the other branch has no pass.
        assert_eq!(compiled.plan.passes().len(), 1, "{}", row.name);
    }
}

#[test]
fn remap_binds_each_branch() {
    let exposure_chain = || {
        vec![
            solid(1, [64, 128, 255, 255]),
            json!({ "id": 2, "type": "exposure", "params": { "exposure": "=frame" },
                    "in": { "source": 1 } }),
        ]
    };
    let remap = json!({ "id": 3, "type": "time_remap", "params": {
        "frame": 8.0, "loop_enabled": false, "loop_start": 0, "loop_end": 0
    }, "in": { "source": 2 } });
    let mut lone = exposure_chain();
    lone.push(remap.clone());
    // The exposure is also read directly, so it is bound once at the remapped frame and once at
    // the bound frame.
    let mut shared = exposure_chain();
    shared.push(remap);
    shared.push(json!({ "id": 4, "type": "merge", "in": { "base": 2, "overlay": 3 } }));

    // Plans are cached across frames, so each row binds at a frame other than its compile frame
    // where it can.
    for (name, nodes, compile_frame, bind_frame, expected) in [
        ("lone", lone, 2, 2, vec![8.0]),
        ("shared", shared, 2, 3, vec![3.0, 8.0]),
    ] {
        let composition = scene((8, 4), json!(nodes));
        let compiled = compile(&composition, Some(compile_frame));
        let buffers = uniform_buffers_of(&compiled.plan, 2);
        assert_eq!(buffers.len(), expected.len(), "{name}");
        let bound = bind(&composition, &compiled, bind_frame);
        let mut values: Vec<f32> = buffers
            .iter()
            .map(|buffer| uploaded_f32s(&bound, *buffer, 1)[0])
            .collect();
        values.sort_by(f32::total_cmp);
        assert_eq!(values, expected, "{name}");
    }
}

#[test]
fn opacity_binds_alpha_flag() {
    let source = solid(1, [64, 128, 255, 128]);
    let unpremultiplied = json!({ "id": 2, "type": "alpha_premultiply",
        "params": { "mode": "unpremultiply" }, "in": { "source": 1 } });
    // The uniform holds the opacity, then 1 if the node scales premultiplied RGB with alpha.
    for (name, nodes, input, alpha_mode, frames) in [
        (
            "premultiplied, animated",
            json!([
                source.clone(),
                { "id": 2, "type": "opacity", "params": { "opacity": "=frame / 10" },
                  "in": { "source": 1 } },
            ]),
            1,
            AlphaMode::Premultiplied,
            vec![(0, [0.0, 1.0]), (5, [0.5, 1.0])],
        ),
        (
            "unpremultiplied",
            json!([
                source.clone(),
                unpremultiplied,
                { "id": 3, "type": "opacity", "params": { "opacity": 0.25 },
                  "in": { "source": 2 } },
            ]),
            2,
            AlphaMode::Unpremultiplied,
            vec![(0, [0.25, 0.0])],
        ),
    ] {
        let composition = scene((8, 4), nodes);
        let compiled = compile(&composition, None);
        let opacity = input + 1;
        let buffer = uniform_buffers_of(&compiled.plan, opacity)[0];
        let (output, source) = (raster(&compiled, opacity), raster(&compiled, input));
        assert_eq!(output.domain, source.domain, "{name}");
        assert_eq!(output.metadata, source.metadata, "{name}");
        assert_eq!(output.metadata.alpha_mode, alpha_mode, "{name}");
        for (frame, expected) in frames {
            let bound = bind(&composition, &compiled, frame);
            assert_eq!(
                uploaded_f32s(&bound, buffer, 2),
                expected,
                "{name}, frame {frame}"
            );
        }
    }
}

enum Stage {
    Compile,
    Bind,
}

struct BindError {
    name: String,
    composition: Composition,
    stage: Stage,
    message: String,
}

fn row(name: &str, nodes: Value, stage: Stage, message: impl Into<String>) -> BindError {
    BindError {
        name: name.to_string(),
        composition: scene((8, 4), nodes),
        stage,
        message: message.into(),
    }
}

fn text(content: &str, params: Value) -> Value {
    let mut node = json!({ "id": 1, "type": "text", "params": { "content": content } });
    for (key, value) in params.as_object().unwrap() {
        node["params"][key] = value.clone();
    }
    node
}

#[test]
fn bind_errors() {
    let property = |node: u64, path: &str| format!("property `{path}` on node {node} ");
    let glyph = |name: &str| format!("`{name}` is only available in a text node's glyph_offset");
    let crop_resize = |crop: [i64; 4], resize: [i64; 4]| {
        let [x, y, width, height] = crop;
        let [resize_x, resize_y, resize_width, resize_height] = resize;
        json!([
            solid(1, [255, 0, 0, 255]),
            { "id": 2, "type": "crop", "in": { "source": 1 },
              "params": { "x": x, "y": y, "width": width, "height": height } },
            { "id": 3, "type": "resize", "in": { "source": 2 }, "params": {
                "x": resize_x, "y": resize_y, "width": resize_width, "height": resize_height,
                "mode": "stretch", "sampling": "linear"
            } },
        ])
    };

    let mut rows = Vec::new();
    // Origins are `i32` and extents `u32` on the GPU.
    let valid = [0, 0, 4, 4];
    for (index, path) in ["x", "y", "width", "height"].into_iter().enumerate() {
        let mut invalid = valid;
        invalid[index] = 1 << 32;
        rows.push(row(
            &format!("crop {path} beyond the GPU range"),
            crop_resize(invalid, valid),
            Stage::Bind,
            property(2, path),
        ));
        rows.push(row(
            &format!("resize {path} beyond the GPU range"),
            crop_resize(valid, invalid),
            Stage::Bind,
            property(3, path),
        ));
    }
    rows.push(row(
        "crop origin below i32::MIN",
        crop_resize([i64::from(i32::MIN) - 1, 0, 4, 4], valid),
        Stage::Bind,
        property(2, "x"),
    ));
    rows.push(BindError {
        name: "alpha_premultiply mode is not an expression".to_string(),
        composition: atlas_composition(|graph| {
            let (source, convert) = (NodeId::new(1), NodeId::new(2));
            graph.nodes.insert(
                source,
                NodeKind::Background(Background {
                    id: source,
                    ..Background::default()
                }),
            );
            graph.nodes.insert(
                convert,
                NodeKind::AlphaPremultiply(AlphaPremultiply {
                    id: convert,
                    params: AlphaPremultiplyParamsDelegate {
                        mode: Deferred::Expr(Expression::parse("\"unpremultiply\"").unwrap()),
                    },
                    source: port(1),
                }),
            );
            convert
        }),
        stage: Stage::Compile,
        message: property(2, "mode"),
    });
    rows.push(row(
        "word outside glyph params",
        json!([text("Lumen", json!({ "position": "=vec2(word * 10, 0)" }))]),
        Stage::Bind,
        glyph("word"),
    ));
    rows.push(row(
        "line outside glyph params",
        json!([text("Lumen", json!({ "opacity": "=line" }))]),
        Stage::Bind,
        glyph("line"),
    ));
    rows.push(row(
        "glyph on a node that is not text",
        json!([{ "id": 1, "type": "shape", "params": { "opacity": "=glyph / glyph_count" } }]),
        Stage::Bind,
        glyph("glyph"),
    ));
    // Another node's property is evaluated outside the glyph, so it fails as it would alone.
    let foreign = |glyph_offset: &str| {
        json!([
            text("Lumen", json!({ "glyph_offset": glyph_offset })),
            { "id": 2, "type": "text", "params": { "content": "Lumen", "font_size": "=20 + glyph * 10" } },
            { "id": 3, "type": "exposure", "in": { "source": 1 } },
        ])
    };
    rows.push(row(
        "foreign property read in glyph scope",
        foreign("=vec2(node(2, \"font_size\"), 0)"),
        Stage::Bind,
        "`glyph` is only available",
    ));
    rows.push(row(
        "text_width of a foreign node in glyph scope",
        foreign("=vec2(text_width(node(2)), 0)"),
        Stage::Bind,
        "`glyph` is only available",
    ));
    let mut data = String::from("M 10 10");
    for index in 1..20_000 - 2 {
        data += &format!(
            " L {} {}",
            10 + index % 100,
            if index % 2 == 0 { 10 } else { 11 }
        );
    }
    rows.push(row(
        "path above the point limit",
        json!([{ "id": 1, "type": "path", "params": { "data": data + " Z" } }]),
        Stage::Bind,
        "more than 16384 points",
    ));

    for row in rows {
        let composition = row.composition;
        let error = match row.stage {
            Stage::Compile => CompileContext::new(&composition)
                .compile()
                .map(|_| ())
                .expect_err(&row.name),
            Stage::Bind => {
                let compiled = compile(&composition, None);
                FrameBindContext::new(&composition, 0)
                    .bind(&compiled)
                    .map(|_| ())
                    .expect_err(&row.name)
            }
        }
        .to_string();
        assert!(error.contains(&row.message), "{}: {error}", row.name);
    }
}

#[test]
fn glyph_scope_crosses_functions() {
    let node = |glyph_offset: Deferred<(f64, f64)>| {
        let mut graph = Graph::new();
        graph.nodes.insert(
            NodeId::new(1),
            NodeKind::Text(Text {
                id: NodeId::new(1),
                params: TextParamsDelegate {
                    content: TextContentDelegate::Text(Deferred::value("Lumen".to_string())),
                    glyph_offset,
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            NodeId::new(2),
            NodeKind::MediaOutput(MediaOutput {
                id: NodeId::new(2),
                source: port(1),
            }),
        );
        CompositionBuilder::new(
            graph,
            TimelineSettings {
                fps: 24.0,
                duration_frames: 12,
            },
            RenderSettings {
                width: 8,
                height: 4,
                background_color: [0, 0, 0, 255],
            },
        )
        .build()
        .unwrap()
    };
    let definitions = std::collections::BTreeMap::from([(
        "slide(distance)".into(),
        "vec2(distance * glyph, 0)".into(),
    )]);
    let _functions = lumen_engine::expr::FunctionScope::enter(&definitions).unwrap();
    let composition = node(Deferred::Expr(Expression::parse("slide(5)").unwrap()));
    let compiled = compile(&composition, None);
    bind(&composition, &compiled, 0);
}

#[test]
fn crop_clamps_extreme_values_to_its_source() {
    // The crop region is clamped to the 8 by 4 source, as origin then size.
    for (name, crop, expected) in [
        (
            "extents beyond i32",
            [0, 0, 1 << 31, i64::from(u32::MAX)],
            [0, 0, 8, 4],
        ),
        (
            "origin at i32::MIN",
            [i64::from(i32::MIN), 0, 4, 4],
            [0, 0, 0, 4],
        ),
        (
            "origin at i32::MAX",
            [i64::from(i32::MAX), 2, 4, 4],
            [8, 2, 0, 2],
        ),
    ] {
        let [x, y, width, height] = crop;
        let composition = scene(
            (8, 4),
            json!([
                solid(1, [255, 0, 0, 255]),
                { "id": 2, "type": "crop", "in": { "source": 1 },
                  "params": { "x": x, "y": y, "width": width, "height": height } },
            ]),
        );
        let compiled = compile(&composition, None);
        let buffer = uniform_buffers_of(&compiled.plan, 2)[0];
        let bound = bind(&composition, &compiled, 0);
        let words: Vec<i32> = uploaded(&bound, buffer)[..16]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| i32::from_ne_bytes(*bytes))
            .collect();
        assert_eq!(words, expected, "{name}");
    }
}

#[test]
fn plan_shape() {
    // A small shape gets a texture and dispatch around its bounds, not the whole canvas.
    let composition = scene(
        (1920, 1080),
        json!([{ "id": 1, "type": "shape", "params": {
            "width": 40, "height": 20, "position": [100, 200],
            "stroke_enabled": true, "stroke_width": 2
        } }]),
    );
    let compiled = compile(&composition, None);
    let shape = raster(&compiled, 1);
    let data = shape.domain.data_rect;
    let texture = compiled
        .plan
        .textures()
        .iter()
        .find(|texture| texture.id == shape.texture)
        .unwrap();
    let dispatch = compiled
        .plan
        .passes()
        .iter()
        .find_map(|pass| match &pass.desc {
            lumen_gpu::PassDesc::Compute(desc)
                if desc.label.as_deref() == Some("shape:1:rasterize") =>
            {
                Some(desc.dispatch)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        shape.domain.display_size(),
        lumen_gpu::Size::new(1920, 1080)
    );
    assert!(data.contains(lumen_gpu::RectI::new(99, 199, 42, 22)));
    assert!(data.width <= 48 && data.height <= 28, "{data:?}");
    assert_eq!(texture.desc.domain.storage_size, data.size());
    assert_eq!(
        dispatch,
        lumen_gpu::Dispatch {
            x: data.width.div_ceil(8),
            y: data.height.div_ceil(8),
            z: 1,
        }
    );
    assert_eq!(
        compiled.output.domain.storage_size,
        lumen_gpu::Size::new(1920, 1080)
    );

    // A merge feather blurs the union of the mask and the base, and a feather that is zero or
    // negative allocates no Gaussian passes.
    let feathered = |mask_size: (u32, u32), feather: Value| {
        let composition = scene(
            (8, 4),
            json!([
                { "id": 1, "type": "background", "params": {
                    "paint": [0, 0, 0, 255], "width": 24, "height": 64 } },
                { "id": 2, "type": "background", "params": {
                    "paint": [255, 255, 255, 255], "width": 24, "height": 64 } },
                { "id": 5, "type": "background", "params": {
                    "paint": [255, 255, 255, 255], "width": mask_size.0, "height": mask_size.1 } },
                { "id": 3, "type": "merge", "params": { "feather": feather },
                  "in": { "base": 1, "overlay": 2, "mask": 5 } },
            ]),
        );
        compile(&composition, None)
    };
    let feather_passes = |compiled: &CompiledComposition| {
        compiled
            .plan
            .passes()
            .iter()
            .filter(|pass| {
                matches!(
                    &pass.desc,
                    lumen_gpu::PassDesc::Compute(desc)
                        if desc.label.as_deref().is_some_and(|label| label.starts_with("merge-feather:"))
                )
            })
            .count()
    };
    let sharp = feathered((24, 64), json!(0.0));
    let negative = feathered((24, 64), json!(-2.0));
    let expression = feathered((24, 64), json!("=0"));
    assert_eq!(feather_passes(&sharp), 0);
    assert_eq!(feather_passes(&negative), 0);
    assert!(feather_passes(&feathered((24, 64), json!(2.0))) > 0);
    assert!(feather_passes(&expression) > 0);
    assert_eq!(negative.plan.textures().len(), sharp.plan.textures().len());
    assert!(expression.plan.textures().len() > sharp.plan.textures().len());

    let feather_size = |compiled: &CompiledComposition| {
        compiled
            .plan
            .textures()
            .iter()
            .find(|texture| texture.label.as_deref() == Some("merge-feather:3:output"))
            .map(|texture| texture.desc.domain.storage_size)
            .unwrap()
    };
    for (name, mask_size, expected) in [
        ("larger mask", (64, 94), (64, 94)),
        ("wider mask", (32, 32), (32, 64)),
        ("smaller mask", (12, 12), (24, 64)),
    ] {
        assert_eq!(
            feather_size(&feathered(mask_size, json!(2.0))),
            lumen_gpu::Size::new(expected.0, expected.1),
            "{name}"
        );
    }
}

fn atlas_composition(nodes: impl FnOnce(&mut Graph) -> NodeId) -> Composition {
    let mut graph = Graph::new();
    let last = nodes(&mut graph);
    let output = NodeId::new(100);
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(last, "output".to_string()),
        }),
    );
    CompositionBuilder::new(
        graph,
        TimelineSettings {
            fps: 24.0,
            duration_frames: 12,
        },
        RenderSettings {
            width: 8,
            height: 4,
            background_color: [0, 0, 0, 255],
        },
    )
    .build()
    .expect("valid test composition")
}

#[test]
fn text_upload_repeats_until_it_is_marked_uploaded() {
    let composition = atlas_composition(|graph| {
        let shape = NodeId::new(1);
        let text = NodeId::new(2);
        let merge = NodeId::new(3);
        graph.nodes.insert(
            shape,
            NodeKind::Shape(Shape {
                id: shape,
                params: ShapeParamsDelegate {
                    width: Deferred::value(6.0),
                    height: Deferred::value(4.0),
                    position: Deferred::value((1.0, 0.0)),
                    fill_paint: Paint::solid([255, 0, 0, 255]).into(),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            text,
            NodeKind::Text(Text {
                id: text,
                params: TextParamsDelegate {
                    content: TextContentDelegate::Text(Deferred::value("hi".to_string())),
                    font_size: Deferred::value(4.0),
                    color: Paint::Gradient(GradientPaint {
                        kind: PaintKind::LinearGradient,
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: [255, 0, 0, 255],
                            },
                            GradientStop {
                                offset: 1.0,
                                color: [0, 0, 255, 255],
                            },
                        ],
                        ..Default::default()
                    })
                    .into(),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            merge,
            NodeKind::Merge(lumen_engine::node::compositing::merge::Merge {
                id: merge,
                base: PortRef::new(shape, "output".to_string()),
                overlay: PortRef::new(text, "output".to_string()),
                ..Default::default()
            }),
        );
        merge
    });
    let compiled = compile(&composition, None);

    let first = bind(&composition, &compiled, 0);
    let first_uploads = first.frame_update().uploads().len();
    assert!(first.texture_upload_count() > 0);

    let rebound = bind(&composition, &compiled, 0);
    assert_eq!(
        rebound.frame_update().uploads().len(),
        first_uploads,
        "text uploads that were never applied must be repeated"
    );
    rebound.mark_uploaded().unwrap();

    let unchanged = bind(&composition, &compiled, 0);
    assert_eq!(unchanged.texture_upload_count(), 0);
    assert!(unchanged.buffer_upload_count() > 0);
}

#[test]
fn text_bind_ahead_restores_the_committed_frame() {
    let composition = atlas_composition(|graph| {
        let text = NodeId::new(1);
        graph.nodes.insert(
            text,
            NodeKind::Text(Text {
                id: text,
                params: TextParamsDelegate {
                    content: TextContentDelegate::Text(Deferred::value("Bind ahead".to_string())),
                    position: Deferred::Expr(Expression::parse("vec2(frame * 10, 0)").unwrap()),
                    ..Default::default()
                },
            }),
        );
        text
    });
    let compiled = compile(&composition, None);
    let first = bind(&composition, &compiled, 0);
    first.mark_uploaded().unwrap();
    bind(&composition, &compiled, 1);
    let restored = bind(&composition, &compiled, 0);
    assert!(restored.buffer_upload_count() > 0);
    assert_eq!(
        restored.texture_upload_count(),
        0,
        "a bind that was never applied must not undo the committed atlas"
    );

    restored.mark_uploaded().unwrap();
    assert_eq!(bind(&composition, &compiled, 0).texture_upload_count(), 0);
    assert!(bind(&composition, &compiled, 1).buffer_upload_count() > 0);
}
