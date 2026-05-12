use lumen::{
    composition::{Composition, RenderSettings, TimelineSettings},
    gpu::{CompileContext, FrameBindContext, FrameBinding},
    graph::{Connection, Graph},
    media::{
        CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver,
        VideoMetadata,
    },
    node::{
        NodeId, NodeKind, NodeProperty, PortRef,
        compositing::{merge::Merge, raster_multimerge::RasterMultiMerge, switch::Switch},
        media_output::MediaOutput,
        processing::{
            alpha_premultiply::AlphaPremultiply, channel_shuffle::ChannelShuffle,
            color_grade::ColorGrade, crop::Crop, exposure::Exposure, hue_saturation::HueSaturation,
            levels::Levels, memo::Memo, resize::Resize, time_remap::TimeRemap,
            transform::Transform,
        },
        source::{media_in::MediaIn, solid_color::SolidColor, text::Text},
        vector::{path::Path, shape::Shape},
    },
};
use std::sync::Arc;

#[test]
fn compiles_solid_color_exposure_media_output_to_gpu_plan() {
    let solid = NodeId::new(1);
    let exposure = NodeId::new(2);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph.nodes.insert(
        solid,
        NodeKind::SolidColor(SolidColor {
            id: solid,
            color: NodeProperty::Color([64, 128, 255, 255]),
            width: NodeProperty::Int(8),
            height: NodeProperty::Int(4),
        }),
    );
    graph.nodes.insert(
        exposure,
        NodeKind::Exposure(Exposure {
            id: exposure,
            exposure: NodeProperty::Float(1.0),
            contrast: NodeProperty::Float(1.0),
            offset: NodeProperty::Float(0.0),
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

    assert_eq!(compiled.plan.textures().len(), 3);
    assert_eq!(compiled.plan.buffers().len(), 2);
    assert_eq!(compiled.plan.programs().len(), 3);
    assert_eq!(compiled.plan.passes().len(), 3);
    assert_eq!(compiled.frame_bindings.len(), 2);
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
        NodeKind::SolidColor(SolidColor {
            id: solid,
            color: NodeProperty::Color([64, 128, 255, 255]),
            width: NodeProperty::Int(8),
            height: NodeProperty::Int(4),
        }),
    );
    graph.nodes.insert(
        exposure,
        NodeKind::Exposure(Exposure {
            id: exposure,
            exposure: NodeProperty::Expr(lumen::expr::Expression::parse("frame / 10").unwrap()),
            contrast: NodeProperty::Float(1.0),
            offset: NodeProperty::Float(0.0),
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

    assert_eq!(compiled.plan.programs().len(), 3);
    assert_eq!(update_zero.uploads().len(), 2);
    assert_eq!(update_ten.uploads().len(), 2);
    assert_ne!(format!("{:?}", update_zero), format!("{:?}", update_ten));
}

#[cfg(feature = "json")]
#[test]
fn compiles_gpu_plan_from_json_composition() {
    let composition = lumen::json::parse(
        r#"{
            "timeline": { "fps": 24, "duration_frames": 12 },
            "render_settings": { "width": 8, "height": 4 },
            "nodes": [
                { "id": 1, "type": "solid_color", "properties": { "color": [64, 128, 255, 255], "width": 8, "height": 4 } },
                { "id": 2, "type": "exposure", "properties": { "exposure": "=frame / 10", "contrast": 1.0, "offset": 0.0 } },
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

    assert_eq!(compiled.plan.textures().len(), 3);
    assert_eq!(compiled.plan.programs().len(), 3);
    assert_eq!(compiled.frame_bindings.len(), 2);
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
            kind: NodeProperty::Int(0),
            source: NodeProperty::String("plate".to_string()),
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

    assert_eq!(compiled.plan.textures().len(), 2);
    assert_eq!(compiled.plan.buffers().len(), 0);
    assert_eq!(compiled.plan.programs().len(), 1);
    assert_eq!(compiled.plan.passes().len(), 1);
    assert_eq!(compiled.frame_bindings.len(), 1);
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
    let merge = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph.nodes.insert(
        base,
        NodeKind::SolidColor(SolidColor {
            id: base,
            width: NodeProperty::Int(1920),
            height: NodeProperty::Int(1080),
            color: NodeProperty::Color([0, 0, 0, 255]),
        }),
    );
    graph.nodes.insert(
        overlay,
        NodeKind::MediaIn(MediaIn {
            id: overlay,
            source: NodeProperty::String("plate".to_string()),
            ..MediaIn::default()
        }),
    );
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            base: PortRef::new(base, "output".to_string()),
            overlay: PortRef::new(overlay, "output".to_string()),
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

    assert_eq!(
        media_raster.domain.storage_size,
        lumen_gpu::Size::new(320, 180)
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
            opacity: NodeProperty::Float(0.5),
            blend_mode: NodeProperty::Int(0),
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

    assert_eq!(compiled.plan.textures().len(), 4);
    assert_eq!(compiled.plan.buffers().len(), 3);
    assert_eq!(compiled.plan.programs().len(), 4);
    assert_eq!(compiled.plan.passes().len(), 4);
    assert_eq!(compiled.frame_bindings.len(), 3);
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
            opacity: NodeProperty::Float(0.5),
            blend_mode: NodeProperty::Int(2),
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

    assert_eq!(compiled.plan.passes().len(), 6);
    assert!(compiled.frame_bindings.iter().any(|binding| {
        matches!(
            binding,
            FrameBinding::RasterMultiMerge {
                node_id,
                blend_mode,
                ..
            } if *node_id == multi && matches!(blend_mode, NodeProperty::Int(2))
        )
    }));
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
            selected_layer: NodeProperty::Int(0),
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

    assert_eq!(compiled.plan.textures().len(), 2);
    assert_eq!(compiled.plan.passes().len(), 2);
    assert_eq!(compiled.frame_bindings.len(), 2);
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
            selected_layer: NodeProperty::Expr(
                lumen::expr::Expression::parse("if(frame < 6, 0, 1)").unwrap(),
            ),
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
fn compiles_memo_and_time_remap_as_gpu_aliases_with_frame_bindings() {
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
            frame: NodeProperty::Float(8.0),
            loop_enabled: NodeProperty::Bool(true),
            loop_start: NodeProperty::Int(4),
            loop_end: NodeProperty::Int(12),
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        memo,
        NodeKind::Memo(Memo {
            id: memo,
            cache_id: NodeProperty::String("cached-comp".to_string()),
            allow_expressions: NodeProperty::Bool(false),
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

    assert_eq!(compiled.plan.textures().len(), 2);
    assert_eq!(compiled.plan.passes().len(), 2);
    assert_eq!(compiled.frame_bindings.len(), 3);
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
            selected_layer: NodeProperty::Expr(
                lumen::expr::Expression::parse("if(frame < 6, 0, 1)").unwrap(),
            ),
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
            frame: NodeProperty::Float(0.0),
            loop_enabled: NodeProperty::Bool(false),
            loop_start: NodeProperty::Int(0),
            loop_end: NodeProperty::Int(0),
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
            exposure: NodeProperty::Expr(lumen::expr::Expression::parse("frame").unwrap()),
            contrast: NodeProperty::Float(1.0),
            offset: NodeProperty::Float(0.0),
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        time_remap,
        NodeKind::TimeRemap(TimeRemap {
            id: time_remap,
            frame: NodeProperty::Float(8.0),
            loop_enabled: NodeProperty::Bool(false),
            loop_start: NodeProperty::Int(0),
            loop_end: NodeProperty::Int(0),
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
        .frame_bindings
        .iter()
        .find_map(|binding| match binding {
            FrameBinding::Exposure {
                node_id, buffer, ..
            } if *node_id == exposure => Some(*buffer),
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
            translate_x: NodeProperty::Expr(lumen::expr::Expression::parse("frame").unwrap()),
            translate_y: NodeProperty::Float(2.0),
            rotate: NodeProperty::Float(15.0),
            source: PortRef::new(solid_id, "output".to_string()),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        crop,
        NodeKind::Crop(Crop {
            id: crop,
            x: NodeProperty::Int(1),
            y: NodeProperty::Int(1),
            width: NodeProperty::Int(6),
            height: NodeProperty::Int(3),
            source: PortRef::new(transform, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        resize,
        NodeKind::Resize(Resize {
            id: resize,
            width: NodeProperty::Int(4),
            height: NodeProperty::Int(2),
            mode: NodeProperty::Int(1),
            sampling: NodeProperty::Int(1),
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
    assert_eq!(compiled.frame_bindings.len(), 4);
    assert!(matches!(
        compiled.frame_bindings[1],
        FrameBinding::Transform { .. }
    ));
    assert!(matches!(
        compiled.frame_bindings[2],
        FrameBinding::Crop { .. }
    ));
    assert!(matches!(
        compiled.frame_bindings[3],
        FrameBinding::Resize { .. }
    ));
    assert_eq!(transformed.domain.storage_size, lumen_gpu::Size::new(8, 4));
    assert_eq!(resized.domain.storage_size, lumen_gpu::Size::new(4, 2));

    FrameBindContext::new(&composition, 7)
        .bind(&compiled)
        .unwrap();
}

#[test]
fn compiles_color_math_nodes_to_gpu_passes_with_frame_bindings() {
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
            mode: NodeProperty::String("unpremultiply".to_string()),
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        shuffle,
        NodeKind::ChannelShuffle(ChannelShuffle {
            id: shuffle,
            red: NodeProperty::String("blue".to_string()),
            green: NodeProperty::String("green".to_string()),
            blue: NodeProperty::String("red".to_string()),
            alpha: NodeProperty::String("0.5".to_string()),
            source: PortRef::new(alpha, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        levels,
        NodeKind::Levels(Levels {
            id: levels,
            black_point: NodeProperty::Float(0.1),
            white_point: NodeProperty::Float(0.9),
            gamma: NodeProperty::Float(1.2),
            output_black: NodeProperty::Float(0.0),
            output_white: NodeProperty::Float(1.0),
            source: PortRef::new(shuffle, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        hue,
        NodeKind::HueSaturation(HueSaturation {
            id: hue,
            hue_degrees: NodeProperty::Expr(lumen::expr::Expression::parse("frame * 10").unwrap()),
            saturation: NodeProperty::Float(0.75),
            lightness: NodeProperty::Float(0.1),
            source: PortRef::new(levels, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        grade,
        NodeKind::ColorGrade(ColorGrade {
            id: grade,
            lut_source: NodeProperty::String("rgb1d: 0,0,0; 255,128,0".to_string()),
            strength: NodeProperty::Float(0.5),
            interpolation: NodeProperty::Int(1),
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

    assert_eq!(compiled.plan.textures().len(), 7);
    assert_eq!(compiled.plan.buffers().len(), 7);
    assert_eq!(compiled.plan.programs().len(), 7);
    assert_eq!(compiled.plan.passes().len(), 7);
    assert_eq!(compiled.plan.params().len(), 7);
    assert_eq!(compiled.frame_bindings.len(), 6);
    assert!(matches!(
        compiled.frame_bindings[1],
        FrameBinding::AlphaPremultiply { .. }
    ));
    assert!(matches!(
        compiled.frame_bindings[2],
        FrameBinding::ChannelShuffle { .. }
    ));
    assert!(matches!(
        compiled.frame_bindings[3],
        FrameBinding::Levels { .. }
    ));
    assert!(matches!(
        compiled.frame_bindings[4],
        FrameBinding::HueSaturation { .. }
    ));
    assert!(matches!(
        compiled.frame_bindings[5],
        FrameBinding::ColorGrade { .. }
    ));
    assert_eq!(update.uploads().len(), 7);
}

#[test]
fn compiles_source_text_and_vector_shape_through_shared_renderer() {
    let shape = NodeId::new(1);
    let text = NodeId::new(2);
    let merge = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            width: NodeProperty::Int(6),
            height: NodeProperty::Int(4),
            position: NodeProperty::Vec2((1.0, 0.0)),
            fill_color: NodeProperty::Color([255, 0, 0, 255]),
            ..Shape::default()
        }),
    );
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            content: NodeProperty::String("hi".to_string()),
            font_size: NodeProperty::Float(4.0),
            color: NodeProperty::Color([255, 255, 255, 255]),
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

    assert_eq!(compiled.plan.textures().len(), 5);
    assert_eq!(compiled.plan.programs().len(), 4);
    assert!(matches!(
        compiled.frame_bindings[0],
        FrameBinding::Shape { .. }
    ));
    assert!(matches!(
        compiled.frame_bindings[1],
        FrameBinding::Text { .. }
    ));
    assert_eq!(bound.frame_update().uploads().len(), 5);

    let unchanged = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap();
    assert_eq!(unchanged.frame_update().uploads().len(), 2);
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
            data: NodeProperty::String("M 1 1 L 6 1 L 6 3 L 1 3 Z".to_string()),
            fill_color: NodeProperty::Color([0, 255, 0, 255]),
            stroke_enabled: NodeProperty::Bool(true),
            stroke_width: NodeProperty::Float(1.0),
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

    assert!(matches!(
        compiled.frame_bindings[0],
        FrameBinding::Path { .. }
    ));
    assert_eq!(bound.frame_update().uploads().len(), 2);
}

fn solid(id: NodeId, color: [u8; 4]) -> NodeKind {
    NodeKind::SolidColor(SolidColor {
        id,
        color: NodeProperty::Color(color),
        width: NodeProperty::Int(8),
        height: NodeProperty::Int(4),
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

    fn frame(&self) -> Result<MediaFrame, lumen::error::MediaError> {
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

    fn frame(&self, _frame: u32) -> Result<MediaFrame, lumen::error::MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.frame)))
    }
}
