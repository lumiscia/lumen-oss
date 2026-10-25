use lumen_engine::{
    composition::{Composition, RenderSettings, TimelineSettings},
    error::{GraphValidationError, LumenError, PropertyError},
    expr::Expression,
    gpu::{AlphaMode, CompileContext, FrameBindContext},
    graph::{Connection, Graph},
    media::{
        CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver,
        VideoMetadata, collect_frame_requirements,
    },
    node::{
        Deferred, NodeId, NodeKind, PortRef,
        compositing::{
            merge::{Merge, MergeParamsDelegate},
            raster_multimerge::{RasterMultiMerge, RasterMultiMergeParamsDelegate},
            switch::{Switch, SwitchParamsDelegate},
        },
        media_output::MediaOutput,
        processing::{
            alpha_premultiply::{AlphaPremultiply, AlphaPremultiplyParamsDelegate},
            channel_shuffle::{ChannelSelector, ChannelShuffle, ChannelShuffleParamsDelegate},
            color_grade::{ColorGrade, ColorGradeParamsDelegate},
            crop::{Crop, CropParamsDelegate},
            exposure::{Exposure, ExposureParamsDelegate},
            hue_saturation::{HueSaturation, HueSaturationParamsDelegate},
            levels::{Levels, LevelsParamsDelegate},
            memo::{Memo, MemoParamsDelegate},
            opacity::{Opacity, OpacityParamsDelegate},
            resize::{Resize, ResizeMode, ResizeParamsDelegate, ResizeSampling},
            shadow::{Shadow, ShadowParamsDelegate},
            time_remap::{TimeRemap, TimeRemapParamsDelegate},
            transform::{Transform, TransformParamsDelegate, TransformSampling},
            wgsl_shader::WgslShader,
        },
        source::{
            background::{Background, BackgroundParamsDelegate},
            media_in::{MediaIn, MediaInParamsDelegate},
            text::{Text, TextAlignmentHorizontal, TextAlignmentVertical, TextParamsDelegate},
        },
        vector::{
            paint::{GradientPaint, GradientStop, Paint, PaintKind},
            path::{Path, PathParamsDelegate},
            shape::{Shape, ShapeParamsDelegate},
        },
    },
};
use std::sync::{Arc, mpsc};

static TEXT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn compiler_rejects_cycle_before_recursive_port_traversal() {
    let first = NodeId::new(1);
    let second = NodeId::new(2);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph.nodes.insert(
        first,
        NodeKind::Exposure(Exposure {
            id: first,
            source: PortRef::new(second, "output".to_string()),
            ..Exposure::default()
        }),
    );
    graph.nodes.insert(
        second,
        NodeKind::Exposure(Exposure {
            id: second,
            source: PortRef::new(first, "output".to_string()),
            ..Exposure::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(first, "output".to_string()),
        }),
    );
    for connection in [
        Connection {
            from_node: first,
            from_port: "output".to_string(),
            to_node: second,
            to_port: "source".to_string(),
        },
        Connection {
            from_node: second,
            from_port: "output".to_string(),
            to_node: first,
            to_port: "source".to_string(),
        },
        Connection {
            from_node: first,
            from_port: "output".to_string(),
            to_node: output,
            to_port: "source".to_string(),
        },
    ] {
        graph.connect(connection).unwrap();
    }
    let composition = Composition::new(
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
    );

    let error = CompileContext::new(&composition)
        .compile()
        .expect_err("cyclic graph must be rejected before compilation");
    assert!(matches!(
        error,
        LumenError::GraphValidation(GraphValidationError::Cycle { .. })
    ));
}

#[test]
fn compiles_background_exposure_media_output_to_gpu_plan() {
    let solid = NodeId::new(1);
    let exposure = NodeId::new(2);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph.nodes.insert(
        solid,
        NodeKind::Background(Background {
            id: solid,
            params: BackgroundParamsDelegate {
                paint: lumen_engine::node::vector::paint::PaintDelegate::from(
                    lumen_engine::node::vector::paint::Paint::solid([64, 128, 255, 255]),
                ),
                width: Deferred::value(8),
                height: Deferred::value(4),
                paint_supersample: Deferred::value(true),
            },
        }),
    );
    graph.nodes.insert(
        exposure,
        NodeKind::Exposure(Exposure {
            id: exposure,
            params: ExposureParamsDelegate {
                exposure: Deferred::value(1.0),
                contrast: Deferred::value(1.0),
                offset: Deferred::value(0.0),
            },
            source: PortRef::new(solid, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(exposure, "output".to_string()),
        }),
    );
    graph
        .connect(Connection {
            from_node: solid,
            from_port: "output".to_string(),
            to_node: exposure,
            to_port: "source".to_string(),
        })
        .unwrap();
    graph
        .connect(Connection {
            from_node: exposure,
            from_port: "output".to_string(),
            to_node: output,
            to_port: "source".to_string(),
        })
        .unwrap();
    let composition = Composition::new(
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
    );

    let compiled = CompileContext::new(&composition).compile().unwrap();

    assert_eq!(compiled.plan.textures().len(), 2);
    assert_eq!(compiled.plan.buffers().len(), 2);
    assert_eq!(compiled.plan.programs().len(), 2);
    assert_eq!(compiled.plan.passes().len(), 2);
    assert_eq!(compiled.compiled_nodes.len(), 2);
    assert_eq!(
        compiled.output.domain.storage_size,
        lumen_gpu::Size::new(8, 4)
    );
}

#[test]
fn frame_binding_updates_expression_uniforms_without_recompile() {
    let solid = NodeId::new(1);
    let exposure = NodeId::new(2);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph.nodes.insert(
        solid,
        NodeKind::Background(Background {
            id: solid,
            params: BackgroundParamsDelegate {
                paint: lumen_engine::node::vector::paint::PaintDelegate::from(
                    lumen_engine::node::vector::paint::Paint::solid([64, 128, 255, 255]),
                ),
                width: Deferred::value(8),
                height: Deferred::value(4),
                paint_supersample: Deferred::value(true),
            },
        }),
    );
    graph.nodes.insert(
        exposure,
        NodeKind::Exposure(Exposure {
            id: exposure,
            params: ExposureParamsDelegate {
                exposure: Deferred::Expr(
                    lumen_engine::expr::Expression::parse("frame / 10").unwrap(),
                ),
                contrast: Deferred::value(1.0),
                offset: Deferred::value(0.0),
            },
            source: PortRef::new(solid, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(exposure, "output".to_string()),
        }),
    );
    graph
        .connect(Connection {
            from_node: solid,
            from_port: "output".to_string(),
            to_node: exposure,
            to_port: "source".to_string(),
        })
        .unwrap();
    graph
        .connect(Connection {
            from_node: exposure,
            from_port: "output".to_string(),
            to_node: output,
            to_port: "source".to_string(),
        })
        .unwrap();
    let composition = Composition::new(
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
    );
    let compiled = CompileContext::new(&composition).compile().unwrap();

    let frame_zero = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();
    let frame_ten = FrameBindContext::new(&composition, 10)
        .bind(&compiled)
        .unwrap();
    let update_zero = frame_zero.frame_update();
    let update_ten = frame_ten.frame_update();

    assert_eq!(compiled.plan.programs().len(), 2);
    assert_eq!(update_zero.uploads().len(), 2);
    assert_eq!(update_ten.uploads().len(), 2);
    assert_ne!(format!("{:?}", update_zero), format!("{:?}", update_ten));
}

#[cfg(feature = "json")]
#[test]
fn compiles_gpu_plan_from_json_composition() {
    let composition = lumen_engine::json::parse(
        r#"{
            "timeline": { "fps": 24, "duration_frames": 12 },
            "render_settings": { "width": 8, "height": 4 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [64, 128, 255, 255], "width": 8, "height": 4 } },
                { "id": 2, "type": "exposure", "params": { "exposure": "=frame / 10", "contrast": 1.0, "offset": 0.0 } },
                { "id": 3, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" },
                { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "source" }
            ]
        }"#,
    )
    .unwrap();

    let compiled = CompileContext::new(&composition).compile().unwrap();

    assert_eq!(compiled.plan.textures().len(), 2);
    assert_eq!(compiled.plan.programs().len(), 2);
    assert_eq!(compiled.compiled_nodes.len(), 2);
}

#[test]
fn compiles_media_input_as_frame_texture_boundary() {
    let media = NodeId::new(1);
    let output = NodeId::new(2);
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
            ..MediaIn::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(media, "output".to_string()),
        }),
    );
    graph
        .connect(Connection {
            from_node: media,
            from_port: "output".to_string(),
            to_node: output,
            to_port: "source".to_string(),
        })
        .unwrap();
    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 24.0,
            duration_frames: 12,
        },
        RenderSettings {
            width: 16,
            height: 9,
            background_color: [0, 0, 0, 255],
        },
    );

    let compiled = CompileContext::new(&composition).compile().unwrap();

    assert_eq!(compiled.plan.textures().len(), 1);
    assert_eq!(compiled.plan.buffers().len(), 0);
    assert_eq!(compiled.plan.programs().len(), 0);
    assert_eq!(compiled.plan.passes().len(), 0);
    assert_eq!(compiled.compiled_nodes.len(), 1);
    assert_eq!(compiled.plan.params().len(), 1);
    assert_eq!(
        compiled.output.domain.storage_size,
        lumen_gpu::Size::new(16, 9)
    );
}

#[test]
fn compiles_media_input_to_native_domain_when_media_metadata_is_available() {
    let base = NodeId::new(1);
    let overlay = NodeId::new(2);
    let transform = NodeId::new(3);
    let merge = NodeId::new(4);
    let output = NodeId::new(5);
    let mut graph = Graph::new();
    graph.nodes.insert(
        base,
        NodeKind::Background(Background {
            id: base,
            params: BackgroundParamsDelegate {
                width: Deferred::value(1920),
                height: Deferred::value(1080),
                paint: lumen_engine::node::vector::paint::PaintDelegate::from(
                    lumen_engine::node::vector::paint::Paint::solid([0, 0, 0, 255]),
                ),
                paint_supersample: Deferred::value(true),
            },
        }),
    );
    graph.nodes.insert(
        overlay,
        NodeKind::MediaIn(MediaIn {
            id: overlay,
            params: MediaInParamsDelegate {
                source: Deferred::value("plate".to_string()),
                ..Default::default()
            },
            ..MediaIn::default()
        }),
    );
    graph.nodes.insert(
        transform,
        NodeKind::Transform(Transform {
            id: transform,
            source: PortRef::new(overlay, "output".to_string()),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            base: PortRef::new(base, "output".to_string()),
            overlay: PortRef::new(transform, "output".to_string()),
            ..Merge::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(merge, "output".to_string()),
        }),
    );

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 24.0,
            duration_frames: 12,
        },
        RenderSettings {
            width: 1920,
            height: 1080,
            background_color: [0, 0, 0, 255],
        },
    );
    let store = TestMediaStore::image("plate", 320, 180);
    let compiled = CompileContext::with_media(
        &composition,
        &store,
        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
    )
    .compile()
    .unwrap();

    let media_raster = compiled
        .node_outputs
        .get(&PortRef::new(overlay, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(overlay, "output")
        .unwrap();
    let merge_raster = compiled
        .node_outputs
        .get(&PortRef::new(merge, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(merge, "output")
        .unwrap();
    let transformed_raster = compiled
        .node_outputs
        .get(&PortRef::new(transform, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(transform, "output")
        .unwrap();

    assert_eq!(
        media_raster.domain.storage_size,
        lumen_gpu::Size::new(320, 180)
    );
    assert_eq!(
        transformed_raster.domain.storage_size,
        lumen_gpu::Size::new(1920, 1080)
    );
    assert_eq!(
        merge_raster.domain.storage_size,
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
fn compiles_merge_to_gpu_blend_pass() {
    let base = NodeId::new(1);
    let overlay = NodeId::new(2);
    let merge = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph.nodes.insert(base, solid(base, [255, 0, 0, 255]));
    graph
        .nodes
        .insert(overlay, solid(overlay, [0, 0, 255, 128]));
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            params: MergeParamsDelegate {
                opacity: Deferred::value(0.5),
                ..Default::default()
            },
            base: PortRef::new(base, "output".to_string()),
            overlay: PortRef::new(overlay, "output".to_string()),
            mask: PortRef::empty(),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(merge, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();

    assert_eq!(compiled.plan.textures().len(), 3);
    assert_eq!(compiled.plan.buffers().len(), 3);
    assert_eq!(compiled.plan.programs().len(), 3);
    assert_eq!(compiled.plan.passes().len(), 3);
    assert_eq!(compiled.compiled_nodes.len(), 3);
    assert!(compiled.plan.passes().iter().any(|pass| {
        matches!(
            &pass.desc,
            lumen_gpu::PassDesc::Compute(desc)
                if desc.label.as_deref() == Some("merge:3:blend")
        )
    }));
}

#[test]
fn compiles_raster_multimerge_with_blend_mode_binding() {
    let first = NodeId::new(1);
    let second = NodeId::new(2);
    let third = NodeId::new(3);
    let multi = NodeId::new(4);
    let output = NodeId::new(5);
    let mut graph = Graph::new();
    graph.nodes.insert(first, solid(first, [255, 0, 0, 255]));
    graph.nodes.insert(second, solid(second, [0, 255, 0, 128]));
    graph.nodes.insert(third, solid(third, [0, 0, 255, 128]));
    graph.nodes.insert(
        multi,
        NodeKind::RasterMultiMerge(RasterMultiMerge {
            id: multi,
            params: RasterMultiMergeParamsDelegate {
                opacity: Deferred::value(0.5),
                blend_mode: Deferred::value(2),
                ..Default::default()
            },
            layers: vec![
                PortRef::new(first, "output".to_string()),
                PortRef::new(second, "output".to_string()),
                PortRef::new(third, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(multi, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();

    assert_eq!(compiled.plan.passes().len(), 5);
    assert!(compiled.compiled_nodes.contains_key(&multi));
}

#[test]
fn compiles_switch_as_selected_gpu_alias() {
    let first = NodeId::new(1);
    let second = NodeId::new(2);
    let switch = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph.nodes.insert(first, solid(first, [255, 0, 0, 255]));
    graph.nodes.insert(second, solid(second, [0, 255, 0, 255]));
    graph.nodes.insert(
        switch,
        NodeKind::Switch(Switch {
            id: switch,
            params: SwitchParamsDelegate {
                selected_layer: Deferred::value(0),
                ..Default::default()
            },
            layers: vec![
                PortRef::new(first, "output".to_string()),
                PortRef::new(second, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(switch, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let selected = compiled
        .node_outputs
        .get(&PortRef::new(first, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(first, "output")
        .unwrap();
    let switched = compiled
        .node_outputs
        .get(&PortRef::new(switch, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(switch, "output")
        .unwrap();

    assert_eq!(compiled.plan.textures().len(), 1);
    assert_eq!(compiled.plan.passes().len(), 1);
    assert_eq!(compiled.compiled_nodes.len(), 2);
    assert_eq!(switched.texture, selected.texture);
}

#[test]
fn compiles_switch_expression_as_frame_selected_gpu_alias() {
    let first = NodeId::new(1);
    let second = NodeId::new(2);
    let switch = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph.nodes.insert(first, solid(first, [255, 0, 0, 255]));
    graph.nodes.insert(second, solid(second, [0, 255, 0, 255]));
    graph.nodes.insert(
        switch,
        NodeKind::Switch(Switch {
            id: switch,
            params: SwitchParamsDelegate {
                selected_layer: Deferred::Expr(
                    lumen_engine::expr::Expression::parse("if(frame < 6, 0, 1)").unwrap(),
                ),
                ..Default::default()
            },
            layers: vec![
                PortRef::new(first, "output".to_string()),
                PortRef::new(second, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(switch, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let frame_zero =
        CompileContext::with_frame(&composition, 0, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
            .compile()
            .unwrap();
    let frame_ten =
        CompileContext::with_frame(&composition, 10, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
            .compile()
            .unwrap();

    let first_output = frame_zero
        .node_outputs
        .get(&PortRef::new(first, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(first, "output")
        .unwrap();
    let frame_zero_switch = frame_zero
        .node_outputs
        .get(&PortRef::new(switch, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(switch, "output")
        .unwrap();
    let second_output = frame_ten
        .node_outputs
        .get(&PortRef::new(second, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(second, "output")
        .unwrap();
    let frame_ten_switch = frame_ten
        .node_outputs
        .get(&PortRef::new(switch, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(switch, "output")
        .unwrap();

    assert_eq!(frame_zero_switch.texture, first_output.texture);
    assert_eq!(frame_ten_switch.texture, second_output.texture);
    assert!(
        !frame_zero
            .node_outputs
            .contains_key(&PortRef::new(second, "output".to_string()))
    );
    assert!(
        !frame_ten
            .node_outputs
            .contains_key(&PortRef::new(first, "output".to_string()))
    );
}

#[test]
fn compiles_memo_and_time_remap_as_gpu_aliases_with_compiled_nodes() {
    let solid_id = NodeId::new(1);
    let time_remap = NodeId::new(2);
    let memo = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [64, 128, 255, 255]));
    graph.nodes.insert(
        time_remap,
        NodeKind::TimeRemap(TimeRemap {
            id: time_remap,
            params: TimeRemapParamsDelegate {
                frame: Deferred::value(8.0),
                loop_enabled: Deferred::value(true),
                loop_start: Deferred::value(4),
                loop_end: Deferred::value(12),
                ..Default::default()
            },
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        memo,
        NodeKind::Memo(Memo {
            id: memo,
            params: MemoParamsDelegate {
                cache_id: Deferred::value("cached-comp".to_string()),
                allow_expressions: Deferred::value(false),
                ..Default::default()
            },
            source: PortRef::new(time_remap, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(memo, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let source = compiled
        .node_outputs
        .get(&PortRef::new(solid_id, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(solid_id, "output")
        .unwrap();
    let remapped = compiled
        .node_outputs
        .get(&PortRef::new(time_remap, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(time_remap, "output")
        .unwrap();
    let memoized = compiled
        .node_outputs
        .get(&PortRef::new(memo, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(memo, "output")
        .unwrap();

    assert_eq!(compiled.plan.textures().len(), 1);
    assert_eq!(compiled.plan.passes().len(), 1);
    assert_eq!(compiled.compiled_nodes.len(), 3);
    assert_eq!(remapped.texture, source.texture);
    assert_eq!(memoized.texture, source.texture);
}

#[test]
fn time_remap_compiles_source_with_remapped_frame_context() {
    let first = NodeId::new(1);
    let second = NodeId::new(2);
    let switch = NodeId::new(3);
    let time_remap = NodeId::new(4);
    let output = NodeId::new(5);
    let mut graph = Graph::new();
    graph.nodes.insert(first, solid(first, [255, 0, 0, 255]));
    graph.nodes.insert(second, solid(second, [0, 255, 0, 255]));
    graph.nodes.insert(
        switch,
        NodeKind::Switch(Switch {
            id: switch,
            params: SwitchParamsDelegate {
                selected_layer: Deferred::Expr(
                    lumen_engine::expr::Expression::parse("if(frame < 6, 0, 1)").unwrap(),
                ),
                ..Default::default()
            },
            layers: vec![
                PortRef::new(first, "output".to_string()),
                PortRef::new(second, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        time_remap,
        NodeKind::TimeRemap(TimeRemap {
            id: time_remap,
            params: TimeRemapParamsDelegate {
                frame: Deferred::value(0.0),
                loop_enabled: Deferred::value(false),
                loop_start: Deferred::value(0),
                loop_end: Deferred::value(0),
                ..Default::default()
            },
            source: PortRef::new(switch, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(time_remap, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled =
        CompileContext::with_frame(&composition, 10, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
            .compile()
            .unwrap();

    let selected = compiled
        .node_outputs
        .get(&PortRef::new(first, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(first, "output")
        .unwrap();
    let remapped = compiled
        .node_outputs
        .get(&PortRef::new(time_remap, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(time_remap, "output")
        .unwrap();

    assert_eq!(remapped.texture, selected.texture);
    assert!(
        !compiled
            .node_outputs
            .contains_key(&PortRef::new(second, "output".to_string()))
    );
}

#[test]
fn time_remap_binds_source_expressions_with_remapped_frame_context() {
    let solid_id = NodeId::new(1);
    let exposure = NodeId::new(2);
    let time_remap = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [64, 128, 255, 255]));
    graph.nodes.insert(
        exposure,
        NodeKind::Exposure(Exposure {
            id: exposure,
            params: ExposureParamsDelegate {
                exposure: Deferred::Expr(lumen_engine::expr::Expression::parse("frame").unwrap()),
                contrast: Deferred::value(1.0),
                offset: Deferred::value(0.0),
            },
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        time_remap,
        NodeKind::TimeRemap(TimeRemap {
            id: time_remap,
            params: TimeRemapParamsDelegate {
                frame: Deferred::value(8.0),
                loop_enabled: Deferred::value(false),
                loop_start: Deferred::value(0),
                loop_end: Deferred::value(0),
                ..Default::default()
            },
            source: PortRef::new(exposure, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(time_remap, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled =
        CompileContext::with_frame(&composition, 2, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
            .compile()
            .unwrap();
    let exposure_buffer = compiled
        .plan
        .params()
        .iter()
        .find_map(|param| match param.target {
            lumen_gpu::ParamTarget::Buffer(buffer)
                if param.key.owner == lumen_gpu::NodeKey(exposure.0) =>
            {
                Some(buffer)
            }
            _ => None,
        })
        .unwrap();
    let bound = FrameBindContext::new(&composition, 2)
        .bind(&compiled)
        .unwrap();
    let update = bound.frame_update();
    let exposure_upload = update
        .uploads()
        .iter()
        .find_map(|upload| match upload {
            lumen_gpu::Upload::Buffer { id, data, .. } if *id == exposure_buffer => Some(*data),
            _ => None,
        })
        .unwrap();
    let exposure_value = f32::from_ne_bytes(exposure_upload[0..4].try_into().unwrap());

    assert_eq!(exposure_value, 8.0);
}

#[test]
fn time_remap_binds_shared_source_once_per_frame_branch() {
    let solid_id = NodeId::new(1);
    let exposure = NodeId::new(2);
    let time_remap = NodeId::new(3);
    let merge = NodeId::new(4);
    let output = NodeId::new(5);
    let exposure_ref = PortRef::new(exposure, "output".to_string());
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [64, 128, 255, 255]));
    graph.nodes.insert(
        exposure,
        NodeKind::Exposure(Exposure {
            id: exposure,
            params: ExposureParamsDelegate {
                exposure: Deferred::Expr(lumen_engine::expr::Expression::parse("frame").unwrap()),
                contrast: Deferred::value(1.0),
                offset: Deferred::value(0.0),
            },
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        time_remap,
        NodeKind::TimeRemap(TimeRemap {
            id: time_remap,
            params: TimeRemapParamsDelegate {
                frame: Deferred::value(8.0),
                loop_enabled: Deferred::value(false),
                loop_start: Deferred::value(0),
                loop_end: Deferred::value(0),
                ..Default::default()
            },
            source: exposure_ref.clone(),
        }),
    );
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            base: exposure_ref,
            overlay: PortRef::new(time_remap, "output".to_string()),
            ..Merge::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(merge, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled =
        CompileContext::with_frame(&composition, 2, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
            .compile()
            .unwrap();
    let exposure_buffers = compiled
        .plan
        .params()
        .iter()
        .filter_map(|param| match param.target {
            lumen_gpu::ParamTarget::Buffer(buffer)
                if param.key.owner == lumen_gpu::NodeKey(exposure.0) =>
            {
                Some(buffer)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(exposure_buffers.len(), 2);

    // Bind at a different frame than the compile frame, as cached plans are reused across frames.
    let bound = FrameBindContext::new(&composition, 3)
        .bind(&compiled)
        .unwrap();
    let update = bound.frame_update();
    let mut exposure_values = exposure_buffers
        .iter()
        .map(|buffer| {
            let data = update
                .uploads()
                .iter()
                .find_map(|upload| match upload {
                    lumen_gpu::Upload::Buffer { id, data, .. } if id == buffer => Some(*data),
                    _ => None,
                })
                .expect("every exposure instance must be bound");
            f32::from_ne_bytes(data[0..4].try_into().unwrap())
        })
        .collect::<Vec<_>>();
    exposure_values.sort_by(f32::total_cmp);

    assert_eq!(exposure_values, [3.0, 8.0]);
}

#[test]
fn opacity_compiles_and_binds_animated_multiplier() {
    let solid_id = NodeId::new(1);
    let opacity = NodeId::new(2);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [64, 128, 255, 128]));
    graph.nodes.insert(
        opacity,
        NodeKind::Opacity(Opacity {
            id: opacity,
            params: OpacityParamsDelegate {
                opacity: Deferred::Expr(
                    lumen_engine::expr::Expression::parse("frame / 10").unwrap(),
                ),
            },
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(opacity, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let opacity_buffer = compiled
        .plan
        .params()
        .iter()
        .find_map(|param| match param.target {
            lumen_gpu::ParamTarget::Buffer(buffer)
                if param.key.owner == lumen_gpu::NodeKey(opacity.0) =>
            {
                Some(buffer)
            }
            _ => None,
        })
        .unwrap();
    let opacity_raster = compiled
        .node_outputs
        .get(&PortRef::new(opacity, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(opacity, "output")
        .unwrap();
    let source_raster = compiled
        .node_outputs
        .get(&PortRef::new(solid_id, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(solid_id, "output")
        .unwrap();
    assert_eq!(opacity_raster.domain, source_raster.domain);
    assert_eq!(opacity_raster.metadata, source_raster.metadata);

    let bound = FrameBindContext::new(&composition, 5)
        .bind(&compiled)
        .unwrap();
    let opacity_upload = bound
        .frame_update()
        .uploads()
        .iter()
        .find_map(|upload| match upload {
            lumen_gpu::Upload::Buffer { id, data, .. } if *id == opacity_buffer => Some(*data),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        f32::from_ne_bytes(opacity_upload[0..4].try_into().unwrap()),
        0.5
    );
    assert_eq!(
        f32::from_ne_bytes(opacity_upload[4..8].try_into().unwrap()),
        1.0,
        "premultiplied RGB must be scaled with alpha",
    );
}

#[test]
fn opacity_preserves_unpremultiplied_rgb_representation() {
    let solid_id = NodeId::new(1);
    let unpremultiply = NodeId::new(2);
    let opacity = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [64, 128, 255, 128]));
    graph.nodes.insert(
        unpremultiply,
        NodeKind::AlphaPremultiply(AlphaPremultiply {
            id: unpremultiply,
            params: AlphaPremultiplyParamsDelegate {
                mode: Deferred::value("unpremultiply".to_string()),
            },
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        opacity,
        NodeKind::Opacity(Opacity {
            id: opacity,
            params: OpacityParamsDelegate {
                opacity: Deferred::value(0.25),
            },
            source: PortRef::new(unpremultiply, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(opacity, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let opacity_buffer = compiled
        .plan
        .params()
        .iter()
        .find_map(|param| match param.target {
            lumen_gpu::ParamTarget::Buffer(buffer)
                if param.key.owner == lumen_gpu::NodeKey(opacity.0) =>
            {
                Some(buffer)
            }
            _ => None,
        })
        .unwrap();
    let opacity_raster = compiled
        .node_outputs
        .get(&PortRef::new(opacity, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(opacity, "output")
        .unwrap();
    assert_eq!(
        opacity_raster.metadata.alpha_mode,
        AlphaMode::Unpremultiplied
    );

    let bound = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();
    let opacity_upload = bound
        .frame_update()
        .uploads()
        .iter()
        .find_map(|upload| match upload {
            lumen_gpu::Upload::Buffer { id, data, .. } if *id == opacity_buffer => Some(*data),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        f32::from_ne_bytes(opacity_upload[0..4].try_into().unwrap()),
        0.25
    );
    assert_eq!(
        f32::from_ne_bytes(opacity_upload[4..8].try_into().unwrap()),
        0.0,
        "unpremultiplied RGB must remain unchanged while alpha is scaled",
    );
}

#[test]
fn opacity_direct_source_is_traversed_for_media_prediction() {
    let media = NodeId::new(1);
    let opacity = NodeId::new(2);
    let output = NodeId::new(3);
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
            source: PortRef::new(media, "output".to_string()),
            ..Opacity::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(opacity, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let requirements =
        collect_frame_requirements(&composition, &TestMediaStore::image("plate", 8, 4), 0).unwrap();

    assert_eq!(requirements.images, vec!["plate"]);
    assert!(requirements.videos.is_empty());
}

#[test]
fn compiles_transform_crop_resize_to_gpu_plan_with_frame_uniforms() {
    let solid_id = NodeId::new(1);
    let transform = NodeId::new(2);
    let crop = NodeId::new(3);
    let resize = NodeId::new(4);
    let output = NodeId::new(5);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [255, 0, 0, 255]));
    graph.nodes.insert(
        transform,
        NodeKind::Transform(Transform {
            id: transform,
            params: TransformParamsDelegate {
                translate_x: Deferred::Expr(
                    lumen_engine::expr::Expression::parse("frame").unwrap(),
                ),
                translate_y: Deferred::value(2.0),
                rotate: Deferred::value(15.0),
                ..Default::default()
            },
            source: PortRef::new(solid_id, "output".to_string()),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        crop,
        NodeKind::Crop(Crop {
            id: crop,
            params: CropParamsDelegate {
                x: Deferred::value(1),
                y: Deferred::value(1),
                width: Deferred::value(6),
                height: Deferred::value(3),
                ..Default::default()
            },
            source: PortRef::new(transform, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        resize,
        NodeKind::Resize(Resize {
            id: resize,
            params: ResizeParamsDelegate {
                width: Deferred::value(4),
                height: Deferred::value(2),
                mode: ResizeMode::Fit.into(),
                sampling: ResizeSampling::Linear.into(),
                ..Default::default()
            },
            source: PortRef::new(crop, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(resize, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let transformed = compiled
        .node_outputs
        .get(&PortRef::new(transform, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(transform, "output")
        .unwrap();
    let resized = compiled
        .node_outputs
        .get(&PortRef::new(resize, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(resize, "output")
        .unwrap();

    assert_eq!(compiled.plan.textures().len(), 5);
    assert_eq!(compiled.plan.buffers().len(), 4);
    assert_eq!(compiled.plan.programs().len(), 5);
    assert_eq!(compiled.plan.passes().len(), 5);
    assert_eq!(compiled.compiled_nodes.len(), 4);
    assert!(compiled.compiled_nodes.contains_key(&transform));
    assert!(compiled.compiled_nodes.contains_key(&crop));
    assert!(compiled.compiled_nodes.contains_key(&resize));
    assert_eq!(transformed.domain.storage_size, lumen_gpu::Size::new(8, 4));
    assert_eq!(resized.domain.storage_size, lumen_gpu::Size::new(4, 2));

    FrameBindContext::new(&composition, 7)
        .bind(&compiled)
        .unwrap();
}

#[test]
fn compiles_color_math_nodes_to_gpu_passes_with_compiled_nodes() {
    let solid_id = NodeId::new(1);
    let alpha = NodeId::new(2);
    let shuffle = NodeId::new(3);
    let levels = NodeId::new(4);
    let hue = NodeId::new(5);
    let grade = NodeId::new(6);
    let output = NodeId::new(7);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [64, 128, 255, 128]));
    graph.nodes.insert(
        alpha,
        NodeKind::AlphaPremultiply(AlphaPremultiply {
            id: alpha,
            params: AlphaPremultiplyParamsDelegate {
                mode: Deferred::value("unpremultiply".to_string()),
                ..Default::default()
            },
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        shuffle,
        NodeKind::ChannelShuffle(ChannelShuffle {
            id: shuffle,
            params: ChannelShuffleParamsDelegate {
                red: ChannelSelector::Blue.into(),
                green: ChannelSelector::Green.into(),
                blue: ChannelSelector::Red.into(),
                alpha: ChannelSelector::One.into(),
                ..Default::default()
            },
            source: PortRef::new(alpha, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        levels,
        NodeKind::Levels(Levels {
            id: levels,
            params: LevelsParamsDelegate {
                black_point: Deferred::value(0.1),
                white_point: Deferred::value(0.9),
                gamma: Deferred::value(1.2),
                output_black: Deferred::value(0.0),
                output_white: Deferred::value(1.0),
                ..Default::default()
            },
            source: PortRef::new(shuffle, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        hue,
        NodeKind::HueSaturation(HueSaturation {
            id: hue,
            params: HueSaturationParamsDelegate {
                hue_degrees: Deferred::Expr(
                    lumen_engine::expr::Expression::parse("frame * 10").unwrap(),
                ),
                saturation: Deferred::value(0.75),
                lightness: Deferred::value(0.1),
                ..Default::default()
            },
            source: PortRef::new(levels, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        grade,
        NodeKind::ColorGrade(ColorGrade {
            id: grade,
            params: ColorGradeParamsDelegate {
                lut_source: Deferred::value("rgb1d: 0,0,0; 255,128,0".to_string()),
                strength: Deferred::value(0.5),
                interpolation: Deferred::value(1),
                ..Default::default()
            },
            source: PortRef::new(hue, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(grade, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let bound = FrameBindContext::new(&composition, 3)
        .bind(&compiled)
        .unwrap();
    let update = bound.frame_update();

    assert_eq!(compiled.plan.textures().len(), 6);
    assert_eq!(compiled.plan.buffers().len(), 7);
    assert_eq!(compiled.plan.programs().len(), 6);
    assert_eq!(compiled.plan.passes().len(), 6);
    assert_eq!(compiled.plan.params().len(), 7);
    assert_eq!(compiled.compiled_nodes.len(), 6);
    assert!(compiled.compiled_nodes.contains_key(&alpha));
    assert!(compiled.compiled_nodes.contains_key(&shuffle));
    assert!(compiled.compiled_nodes.contains_key(&levels));
    assert!(compiled.compiled_nodes.contains_key(&hue));
    assert!(compiled.compiled_nodes.contains_key(&grade));
    assert_eq!(update.uploads().len(), 7);
}

#[test]
fn compiles_source_text_and_vector_shape_through_shared_renderer() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let shape = NodeId::new(1);
    let text = NodeId::new(2);
    let merge = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            params: ShapeParamsDelegate {
                width: Deferred::value(6),
                height: Deferred::value(4),
                position: Deferred::value((1.0, 0.0)),
                fill_paint: Paint::solid([255, 0, 0, 255]).into(),
                ..Default::default()
            },
            ..Shape::default()
        }),
    );
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            params: TextParamsDelegate {
                content: Deferred::value("hi".to_string()),
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
            ..Text::default()
        }),
    );
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            base: PortRef::new(shape, "output".to_string()),
            overlay: PortRef::new(text, "output".to_string()),
            ..Merge::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(merge, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let bound = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();

    assert_eq!(compiled.plan.textures().len(), 4);
    assert_eq!(compiled.plan.buffers().len(), 5);
    assert_eq!(compiled.plan.programs().len(), 3);
    assert!(compiled.compiled_nodes.contains_key(&shape));
    assert!(compiled.compiled_nodes.contains_key(&text));
    let first_uploads = bound.frame_update().uploads().len();

    let rebound = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();
    assert_eq!(
        rebound.frame_update().uploads().len(),
        first_uploads,
        "text uploads that were never applied must be repeated"
    );
    rebound.mark_uploaded().unwrap();

    let unchanged = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();
    assert_eq!(unchanged.frame_update().uploads().len(), 2);
}

#[test]
fn text_bind_ahead_restores_committed_frame_after_pending_frame() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let text = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            params: TextParamsDelegate {
                content: Deferred::value("Bind ahead".to_string()),
                position: Deferred::Expr(Expression::parse("vec2(frame * 10, 0)").unwrap()),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(text, "output".to_string()),
        }),
    );
    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let bind = |frame| {
        FrameBindContext::new(&composition, frame)
            .bind(&compiled)
            .unwrap()
    };
    let first = bind(0);
    first.mark_uploaded().unwrap();
    bind(1);
    let restored = bind(0);
    assert!(restored.buffer_upload_count() > 0);

    restored.mark_uploaded().unwrap();
    assert_eq!(bind(0).buffer_upload_count(), 0);
    assert!(bind(1).buffer_upload_count() > 0);
}

#[test]
fn text_rejects_stale_upload_after_atlas_slot_reuse() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            params: TextParamsDelegate {
                content: Deferred::Expr(Expression::parse("if(frame == 0, \"W\", \"M\")").unwrap()),
                font_size: Deferred::value(2000.0),
                position: Deferred::value((0.0, -400.0)),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(text, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = 512;
    composition.render_settings.height = 512;
    let older = renderer
        .bind_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    let current = renderer
        .bind_frame(&composition, 1, &EmptyMediaStore)
        .unwrap();
    assert!(older.texture_upload_count() > 0);
    assert!(current.texture_upload_count() > 0);
    renderer.upload_bound_frame(&current).unwrap();
    let expected = render_rgba8(&mut renderer, &composition, 1, &EmptyMediaStore);
    assert!(expected.chunks_exact(4).any(|pixel| pixel[3] > 0));
    let error = renderer.upload_bound_frame(&older).unwrap_err();
    assert!(error.to_string().contains("stale bound text frame"));
    let rebound = renderer
        .bind_frame(&composition, 1, &EmptyMediaStore)
        .unwrap();
    assert_eq!(rebound.texture_upload_count(), 0);
    renderer.submit_bound_frame(&rebound).unwrap();
    assert_eq!(
        render_rgba8(&mut renderer, &composition, 1, &EmptyMediaStore),
        expected
    );
}

#[test]
fn text_rejects_stale_upload_from_another_compiled_plan() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = NodeId::new(1);
    let first = NodeId::new(2);
    let second = NodeId::new(3);
    let switch = NodeId::new(4);
    let merge = NodeId::new(5);
    let output = NodeId::new(6);
    let mut graph = Graph::new();
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            params: TextParamsDelegate {
                content: Deferred::Expr(Expression::parse("if(frame == 0, \"W\", \"M\")").unwrap()),
                font_size: Deferred::value(32.0),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(first, solid(first, [0; 4]));
    graph.nodes.insert(second, solid(second, [0; 4]));
    graph.nodes.insert(
        switch,
        NodeKind::Switch(Switch {
            id: switch,
            params: SwitchParamsDelegate {
                selected_layer: Deferred::Expr(Expression::parse("frame").unwrap()),
            },
            layers: vec![
                PortRef::new(first, "output".to_string()),
                PortRef::new(second, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            base: PortRef::new(text, "output".to_string()),
            overlay: PortRef::new(switch, "output".to_string()),
            ..Default::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(merge, "output".to_string()),
        }),
    );
    for (from_node, to_port) in [(text, "base"), (switch, "overlay")] {
        graph
            .connect(Connection {
                from_node,
                from_port: "output".to_string(),
                to_node: merge,
                to_port: to_port.to_string(),
            })
            .unwrap();
    }
    let mut composition = test_composition(graph);
    composition.render_settings.width = 64;
    composition.render_settings.height = 64;
    let older = renderer
        .bind_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    let current = renderer
        .bind_frame(&composition, 1, &EmptyMediaStore)
        .unwrap();
    renderer.upload_bound_frame(&current).unwrap();
    let error = renderer.upload_bound_frame(&older).unwrap_err();
    assert!(error.to_string().contains("stale bound text frame"));
    let rebound = renderer
        .bind_frame(&composition, 1, &EmptyMediaStore)
        .unwrap();
    assert_eq!(rebound.texture_upload_count(), 0);
    assert_eq!(rebound.buffer_upload_count(), 2);
    let pixels = render_rgba8(&mut renderer, &composition, 1, &EmptyMediaStore);
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
    let mut reference = gpu_composition_renderer().unwrap();
    assert_eq!(
        pixels,
        render_rgba8(&mut reference, &composition, 1, &EmptyMediaStore)
    );
}

#[test]
fn text_measurement_loads_time_remapped_family_on_first_bind() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    #[derive(Debug, Clone)]
    struct FontStore(Vec<u8>);

    impl lumen_engine::media::FontResolver for FontStore {
        fn id(&self) -> &str {
            "remapped-text-test"
        }

        fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
            Ok(vec![self.0.clone()])
        }
    }

    impl MediaStore for FontStore {
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }

        fn get_font_resolver(
            &self,
            family: &str,
        ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
            (family == "RemapX").then(|| Box::new(self.clone()) as _)
        }
    }

    // Give the fixture a unique family and half-size metrics to distinguish fallback.
    let mut font = lumen_text::ROBOTO_REGULAR_BYTES.to_vec();
    for (old, new) in [
        (b"Roboto".as_slice(), b"RemapX".as_slice()),
        (
            b"\0R\0o\0b\0o\0t\0o".as_slice(),
            b"\0R\0e\0m\0a\0p\0X".as_slice(),
        ),
    ] {
        for offset in 0..=font.len() - old.len() {
            if &font[offset..offset + old.len()] == old {
                font[offset..offset + old.len()].copy_from_slice(new);
            }
        }
    }
    let table_count = u16::from_be_bytes(font[4..6].try_into().unwrap()) as usize;
    let head = font[12..12 + table_count * 16]
        .chunks_exact(16)
        .find(|table| &table[..4] == b"head")
        .unwrap();
    let units_offset = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize + 18;
    let units = u16::from_be_bytes(font[units_offset..units_offset + 2].try_into().unwrap());
    font[units_offset..units_offset + 2].copy_from_slice(&(units * 2).to_be_bytes());
    let store = FontStore(font.clone());
    let mut request = lumen_text::TextLayoutRequest::new("WWW");
    request.font_family = "RemapX".to_string();
    request.font_size = 32.0;
    let mut reference = lumen_text::TextSystem::new();
    let fallback_width = reference.measure(&request).width;
    reference.load_font_data(font);
    let expected_width = reference.measure(&request).width;
    assert!(expected_width < fallback_width * 0.75);

    let text = NodeId::new(1);
    let remap = NodeId::new(2);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            params: TextParamsDelegate {
                content: Deferred::value("WWW".to_string()),
                font_family: Deferred::Expr(
                    Expression::parse("if(frame == 1, \"RemapX\", \"Roboto\")").unwrap(),
                ),
                font_size: Deferred::value(32.0),
                position: Deferred::Expr(
                    Expression::parse("vec2(text_width(node(1)), text_height(node(1)))").unwrap(),
                ),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        remap,
        NodeKind::TimeRemap(TimeRemap {
            id: remap,
            source: PortRef::new(text, "output".to_string()),
            params: TimeRemapParamsDelegate {
                frame: Deferred::value(1.0),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(remap, "output".to_string()),
        }),
    );
    let composition = test_composition(graph);
    let compiled = CompileContext::with_media(
        &composition,
        &store,
        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
    )
    .compile()
    .unwrap();
    let bind = || {
        FrameBindContext::with_media(&composition, 0, &store)
            .bind(&compiled)
            .unwrap()
    };
    let first = bind();
    let second = bind();
    let instance_buffer = compiled
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some("text:1:instances"))
        .unwrap()
        .id;
    let first_rect = |bound: &lumen_engine::gpu::BoundFrame| -> [f32; 4] {
        bound
            .frame_update()
            .uploads()
            .iter()
            .find_map(|upload| match upload {
                lumen_gpu::Upload::Buffer { id, data, .. } if *id == instance_buffer => {
                    Some(bytemuck::pod_read_unaligned(&data[..16]))
                }
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(first_rect(&first), first_rect(&second));
    assert!((first_rect(&first)[0] - expected_width).abs() < 3.0);
}

#[test]
fn text_at_fractional_position_draws_glyphs_on_whole_pixels() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let text = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            params: TextParamsDelegate {
                content: Deferred::value("Sharp text\nwraps across lines".to_string()),
                font_size: Deferred::value(17.0),
                max_width: Deferred::value(90.0),
                position: Deferred::value((10.3, 20.6)),
                alignment_horizontal: TextAlignmentHorizontal::Center.into(),
                alignment_vertical: TextAlignmentVertical::Middle.into(),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(text, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = 160;
    composition.render_settings.height = 80;

    let compiled = CompileContext::new(&composition).compile().unwrap();
    let bound = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();
    let instances_label = format!("text:{}:instances", text.0);
    let instances_buffer = compiled
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some(instances_label.as_str()))
        .unwrap()
        .id;
    let update = bound.frame_update();
    let instances = update
        .uploads()
        .iter()
        .find_map(|upload| match upload {
            lumen_gpu::Upload::Buffer { id, data, .. } if *id == instances_buffer => Some(*data),
            _ => None,
        })
        .unwrap();
    let rects: Vec<[f32; 4]> = instances
        .chunks_exact(std::mem::size_of::<lumen_text::GpuGlyphInstance>())
        .map(|instance| bytemuck::pod_read_unaligned(&instance[..16]))
        .collect();

    assert!(rects.len() > 10);
    assert!(
        rects
            .iter()
            .all(|rect| rect[0].fract() == 0.0 && rect[1].fract() == 0.0),
        "glyph quads must start on whole pixels: {rects:?}"
    );
}

#[test]
fn compiles_vector_path_through_shared_renderer() {
    let path = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        path,
        NodeKind::Path(Path {
            id: path,
            params: PathParamsDelegate {
                data: Deferred::value("M 1 1 L 6 1 L 6 3 L 1 3 Z".to_string()),
                fill_paint: Paint::solid([0, 255, 0, 255]).into(),
                stroke_enabled: Deferred::value(true),
                stroke_width: Deferred::value(1.0),
                ..Default::default()
            },
            ..Path::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(path, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let bound = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();

    assert!(compiled.compiled_nodes.contains_key(&path));
    assert_eq!(bound.frame_update().uploads().len(), 2);
}

#[test]
fn path_fill_pixels_are_independent_of_stroke_trim_range() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = filled_path_composition();
    let media = EmptyMediaStore;

    let full_pixels = render_rgba8(&mut renderer, &composition, 0, &media);
    let empty_pixels = render_rgba8(&mut renderer, &composition, 1, &media);

    assert_eq!(
        full_pixels, empty_pixels,
        "stroke trim parameters must not change fill coverage or antialiasing"
    );
    assert!(
        full_pixels
            .chunks_exact(4)
            .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
        "subpixel path should exercise antialiased fill coverage"
    );
}

#[test]
fn identity_transform_with_linear_sampling_reproduces_input() {
    let gradient = NodeId::new(1);
    let transform = NodeId::new(2);
    let output = NodeId::new(3);
    let source = || {
        NodeKind::Background(Background {
            id: gradient,
            params: BackgroundParamsDelegate {
                paint: Paint::Gradient(GradientPaint {
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
                width: Deferred::value(8),
                height: Deferred::value(4),
                paint_supersample: Deferred::value(false),
            },
        })
    };
    let render = |with_transform: bool| {
        let mut graph = Graph::new();
        graph.nodes.insert(gradient, source());
        if with_transform {
            graph.nodes.insert(
                transform,
                NodeKind::Transform(Transform {
                    id: transform,
                    params: TransformParamsDelegate {
                        sampling: TransformSampling::Linear.into(),
                        ..Default::default()
                    },
                    source: PortRef::new(gradient, "output".to_string()),
                }),
            );
        }
        graph.nodes.insert(
            output,
            NodeKind::MediaOutput(MediaOutput {
                id: output,
                source: PortRef::new(
                    if with_transform { transform } else { gradient },
                    "output".to_string(),
                ),
            }),
        );
        // The renderer caches compiled plans, so each composition gets its own.
        let mut renderer = gpu_composition_renderer()?;
        Some(render_rgba8(
            &mut renderer,
            &test_composition(graph),
            0,
            &EmptyMediaStore,
        ))
    };
    let Some(input) = render(false) else {
        return;
    };
    let Some(transformed) = render(true) else {
        return;
    };

    assert_eq!(transformed, input);
}

#[test]
fn linear_resize_keeps_source_edges_opaque_and_identity_reproduces_input() {
    let background = NodeId::new(1);
    let resize = NodeId::new(2);
    let output = NodeId::new(3);
    let render = |(source_width, source_height): (u32, u32), use_resize: bool| {
        let mut graph = Graph::new();
        graph.nodes.insert(
            background,
            NodeKind::Background(Background {
                id: background,
                params: BackgroundParamsDelegate {
                    paint: Paint::Gradient(GradientPaint {
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
                    width: Deferred::value(source_width),
                    height: Deferred::value(source_height),
                    paint_supersample: Deferred::value(false),
                },
            }),
        );
        if use_resize {
            graph.nodes.insert(
                resize,
                NodeKind::Resize(Resize {
                    id: resize,
                    params: ResizeParamsDelegate {
                        width: Deferred::value(8),
                        height: Deferred::value(4),
                        mode: ResizeMode::Stretch.into(),
                        sampling: ResizeSampling::Linear.into(),
                    },
                    source: PortRef::new(background, "output".to_string()),
                }),
            );
        }
        graph.nodes.insert(
            output,
            NodeKind::MediaOutput(MediaOutput {
                id: output,
                source: PortRef::new(
                    if use_resize { resize } else { background },
                    "output".to_string(),
                ),
            }),
        );
        let mut renderer = gpu_composition_renderer()?;
        Some(render_rgba8(
            &mut renderer,
            &test_composition(graph),
            0,
            &EmptyMediaStore,
        ))
    };

    let Some(input) = render((8, 4), false) else {
        return;
    };
    let Some(identity) = render((8, 4), true) else {
        return;
    };
    assert_eq!(identity, input);

    // The 4x2 source maps the bottom-right output centre to (3.75, 1.75).
    let Some(upscaled) = render((4, 2), true) else {
        return;
    };
    assert_eq!(upscaled.len(), 8 * 4 * 4);
    assert!(upscaled.chunks_exact(4).all(|pixel| pixel[3] == 255));
}

#[test]
fn merge_and_adjustments_treat_rasters_as_premultiplied() {
    let output = NodeId::new(100);
    let with_output = |mut graph: Graph, source: NodeId| {
        graph.nodes.insert(
            output,
            NodeKind::MediaOutput(MediaOutput {
                id: output,
                source: PortRef::new(source, "output".to_string()),
            }),
        );
        test_composition(graph)
    };
    let merge_over_black = |mut graph: Graph, overlay: NodeId| {
        let base = NodeId::new(50);
        let merge = NodeId::new(51);
        graph.nodes.insert(base, solid(base, [0, 0, 0, 255]));
        graph.nodes.insert(
            merge,
            NodeKind::Merge(Merge {
                id: merge,
                params: MergeParamsDelegate::default(),
                base: PortRef::new(base, "output".to_string()),
                overlay: PortRef::new(overlay, "output".to_string()),
                mask: PortRef::empty(),
            }),
        );
        with_output(graph, merge)
    };
    let straight_half_red = || {
        let (solid_id, unpremultiply) = (NodeId::new(1), NodeId::new(2));
        let mut graph = Graph::new();
        graph
            .nodes
            .insert(solid_id, solid(solid_id, [255, 0, 0, 128]));
        graph.nodes.insert(
            unpremultiply,
            NodeKind::AlphaPremultiply(AlphaPremultiply {
                id: unpremultiply,
                params: AlphaPremultiplyParamsDelegate {
                    mode: Deferred::value("unpremultiply".to_string()),
                },
                source: PortRef::new(solid_id, "output".to_string()),
            }),
        );
        (graph, unpremultiply)
    };
    let assert_pixels = |composition: Composition, expected: [u8; 4], message: &str| {
        // The renderer caches compiled plans, so each composition gets its own.
        let Some(mut renderer) = gpu_composition_renderer() else {
            return;
        };
        let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
        for pixel in pixels.chunks_exact(4) {
            assert!(
                pixel
                    .iter()
                    .zip(expected)
                    .all(|(channel, expected)| channel.abs_diff(expected) <= 1),
                "{message}: expected {expected:?}, got {pixel:?}"
            );
        }
    };

    let white = NodeId::new(1);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(white, solid(white, [255, 255, 255, 128]));
    assert_pixels(
        merge_over_black(graph, white),
        [128, 128, 128, 255],
        "50% white over opaque black should be 50% grey",
    );

    let (graph, straight) = straight_half_red();
    assert_pixels(
        merge_over_black(graph, straight),
        [128, 0, 0, 255],
        "straight-alpha overlay must be premultiplied before compositing",
    );

    let (mut graph, straight) = straight_half_red();
    let shader = NodeId::new(3);
    graph.nodes.insert(
        shader,
        NodeKind::WgslShader(WgslShader {
            id: shader,
            source: PortRef::new(straight, "output".to_string()),
            ..WgslShader::default()
        }),
    );
    assert_pixels(
        with_output(graph, shader),
        [255, 0, 0, 128],
        "default WGSL shader with zero tint must preserve straight alpha",
    );

    let (transparent, exposure) = (NodeId::new(1), NodeId::new(2));
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(transparent, solid(transparent, [0, 0, 0, 0]));
    graph.nodes.insert(
        exposure,
        NodeKind::Exposure(Exposure {
            id: exposure,
            params: ExposureParamsDelegate {
                exposure: Deferred::value(1.0),
                offset: Deferred::value(0.5),
                ..Default::default()
            },
            source: PortRef::new(transparent, "output".to_string()),
        }),
    );
    assert_pixels(
        with_output(graph, exposure),
        [0, 0, 0, 0],
        "adjusting a fully transparent pixel must keep it transparent black",
    );
}

#[test]
fn shadow_composites_straight_alpha_source_in_premultiplied_space() {
    let (solid_id, unpremultiply, shadow, base, merge, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
        NodeId::new(5),
        NodeId::new(6),
    );
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [255, 0, 0, 128]));
    graph.nodes.insert(
        unpremultiply,
        NodeKind::AlphaPremultiply(AlphaPremultiply {
            id: unpremultiply,
            params: AlphaPremultiplyParamsDelegate {
                mode: Deferred::value("unpremultiply".to_string()),
            },
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        shadow,
        NodeKind::Shadow(Shadow {
            id: shadow,
            params: ShadowParamsDelegate {
                offset_x: Deferred::value(0.0),
                offset_y: Deferred::value(0.0),
                radius: Deferred::value(0.0),
                color: Paint::solid([0, 255, 0, 255]).into(),
                opacity: Deferred::value(1.0),
            },
            source: PortRef::new(unpremultiply, "output".to_string()),
        }),
    );
    graph.nodes.insert(base, solid(base, [0, 0, 0, 255]));
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            params: MergeParamsDelegate::default(),
            base: PortRef::new(base, "output".to_string()),
            overlay: PortRef::new(shadow, "output".to_string()),
            mask: PortRef::empty(),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(merge, "output".to_string()),
        }),
    );

    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let pixels = render_rgba8(&mut renderer, &test_composition(graph), 0, &EmptyMediaStore);
    // Premultiplied red (0.5, 0, 0) over half-covered green shadow (0, 0.25, 0)
    // on opaque black.
    for pixel in pixels.chunks_exact(4) {
        assert!(
            pixel
                .iter()
                .zip([128u8, 64, 0, 255])
                .all(|(channel, expected)| channel.abs_diff(expected) <= 1),
            "shadow must composite in premultiplied space: got {pixel:?}"
        );
    }
}

#[test]
fn alpha_premultiply_rejects_expression_mode() {
    let solid_id = NodeId::new(1);
    let convert = NodeId::new(2);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(solid_id, solid(solid_id, [255, 0, 0, 128]));
    graph.nodes.insert(
        convert,
        NodeKind::AlphaPremultiply(AlphaPremultiply {
            id: convert,
            params: AlphaPremultiplyParamsDelegate {
                mode: Deferred::Expr(Expression::parse("\"unpremultiply\"").unwrap()),
            },
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(convert, "output".to_string()),
        }),
    );

    let composition = test_composition(graph);
    let error = CompileContext::new(&composition).compile().unwrap_err();
    assert!(
        matches!(
            error,
            LumenError::Property(PropertyError::InvalidType { node_id, .. }) if node_id == convert
        ),
        "unexpected error: {error:?}"
    );
}

fn filled_path_composition() -> Composition {
    let path = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        path,
        NodeKind::Path(Path {
            id: path,
            params: PathParamsDelegate {
                data: Deferred::value(
                    "M 1.25 0.75 L 6.75 0.75 L 6.75 3.25 L 1.25 3.25 Z".to_string(),
                ),
                fill_paint: Paint::solid([255, 255, 255, 255]).into(),
                stroke_enabled: Deferred::value(false),
                trim_start: Deferred::Expr(Expression::parse("frame * 0.5").unwrap()),
                trim_end: Deferred::Expr(Expression::parse("1 - frame * 0.5").unwrap()),
                edge_antialias: Deferred::value(true),
                ..Default::default()
            },
            ..Path::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(path, "output".to_string()),
        }),
    );
    test_composition(graph)
}

#[derive(Debug)]
struct EmptyMediaStore;

impl MediaStore for EmptyMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}

fn gpu_composition_renderer() -> Option<lumen_engine::gpu::GpuCompositionRenderer> {
    match pollster::block_on(lumen_engine::gpu::GpuCompositionRenderer::new()) {
        Ok(renderer) => Some(renderer),
        Err(error) => {
            eprintln!("skipping GPU-backed lumen-engine test: {error:#}");
            None
        }
    }
}

fn render_rgba8(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    composition: &Composition,
    frame: u32,
    media: &EmptyMediaStore,
) -> Vec<u8> {
    let raster = renderer.render_frame(composition, frame, media).unwrap();
    let gpu = renderer.gpu_renderer();
    let size = raster.domain.storage_size;
    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = size.width * bytes_per_pixel;
    let padded_bytes_per_row = unpadded_bytes_per_row
        .div_ceil(lumen_gpu::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * lumen_gpu::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let output = gpu
        .device
        .create_buffer(&lumen_gpu::wgpu::BufferDescriptor {
            label: Some("lumen-engine path fill test readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(size.height),
            usage: lumen_gpu::wgpu::BufferUsages::COPY_DST
                | lumen_gpu::wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    let mut encoder =
        gpu.device
            .create_command_encoder(&lumen_gpu::wgpu::CommandEncoderDescriptor {
                label: Some("lumen-engine path fill test readback encoder"),
            });
    encoder.copy_texture_to_buffer(
        gpu.texture(raster.texture).unwrap().as_image_copy(),
        lumen_gpu::wgpu::TexelCopyBufferInfo {
            buffer: &output,
            layout: lumen_gpu::wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size.height),
            },
        },
        size.as_extent(),
    );
    gpu.queue.submit([encoder.finish()]);

    let slice = output.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(lumen_gpu::wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    gpu.device
        .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let mut pixels = vec![0; unpadded_bytes_per_row as usize * size.height as usize];
    for row in 0..size.height as usize {
        let source = row * padded_bytes_per_row as usize;
        let destination = row * unpadded_bytes_per_row as usize;
        pixels[destination..destination + unpadded_bytes_per_row as usize]
            .copy_from_slice(&mapped[source..source + unpadded_bytes_per_row as usize]);
    }
    drop(mapped);
    output.unmap();
    pixels
}

fn solid(id: NodeId, color: [u8; 4]) -> NodeKind {
    NodeKind::Background(Background {
        id,
        params: BackgroundParamsDelegate {
            paint: lumen_engine::node::vector::paint::PaintDelegate::from(
                lumen_engine::node::vector::paint::Paint::solid(color),
            ),
            width: Deferred::value(8),
            height: Deferred::value(4),
            paint_supersample: Deferred::value(true),
        },
    })
}

fn test_composition(graph: Graph) -> Composition {
    Composition::new(
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
}

#[derive(Debug, Clone)]
struct TestMediaStore {
    id: String,
    frame: Arc<CpuMediaFrame>,
}

impl TestMediaStore {
    fn image(id: &str, width: u32, height: u32) -> Self {
        let bytes = vec![255; width as usize * height as usize * 4];
        Self {
            id: id.to_string(),
            frame: Arc::new(CpuMediaFrame {
                rgba: Arc::new(bytes),
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

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        (stream_id == self.id).then(|| Box::new(self.clone()) as Box<dyn VideoFrameResolver>)
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

impl VideoFrameResolver for TestMediaStore {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> VideoMetadata {
        VideoMetadata {
            width: self.frame.width,
            height: self.frame.height,
            frame_count: 1,
            fps: 24.0,
        }
    }

    fn frame(&self, _frame: u32) -> Result<MediaFrame, lumen_engine::error::MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.frame)))
    }
}
