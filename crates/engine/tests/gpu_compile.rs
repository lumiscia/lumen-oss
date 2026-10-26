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
            shape::{Shape, ShapeGeometryKind, ShapeParamsDelegate},
        },
    },
};
use std::sync::{Arc, mpsc};

static TEXT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// Vulkan devices from separate wgpu instances can still crash inside the loader when used from
// several threads at once, even with HAL labels discarded, so GPU tests run one at a time. The
// lock is reentrant per thread for tests that hold two renderers, and is released only when the
// last holder on that thread drops.
static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    static GPU_LOCK_HELD: std::cell::RefCell<(usize, Option<std::sync::MutexGuard<'static, ()>>)> =
        const { std::cell::RefCell::new((0, None)) };
}

/// Not `Send`: the count and guard it releases belong to the thread that acquired it.
struct GpuTestLock(std::marker::PhantomData<std::rc::Rc<()>>);

impl GpuTestLock {
    fn acquire() -> Self {
        GPU_LOCK_HELD.with(|held| {
            let mut held = held.borrow_mut();
            if held.0 == 0 {
                held.1 = Some(GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner()));
            }
            held.0 += 1;
        });
        Self(std::marker::PhantomData)
    }
}

impl Drop for GpuTestLock {
    fn drop(&mut self) {
        GPU_LOCK_HELD.with(|held| {
            let mut held = held.borrow_mut();
            held.0 -= 1;
            if held.0 == 0 {
                held.1 = None;
            }
        });
    }
}

struct TestRenderer {
    renderer: lumen_engine::gpu::GpuCompositionRenderer,
    _lock: GpuTestLock,
}

impl std::ops::Deref for TestRenderer {
    type Target = lumen_engine::gpu::GpuCompositionRenderer;

    fn deref(&self) -> &Self::Target {
        &self.renderer
    }
}

impl std::ops::DerefMut for TestRenderer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.renderer
    }
}

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
            },
            source: PortRef::new(transform, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        resize,
        NodeKind::Resize(Resize {
            id: resize,
            params: ResizeParamsDelegate {
                x: Deferred::value(0),
                y: Deferred::value(0),
                width: Deferred::value(4),
                height: Deferred::value(2),
                mode: ResizeMode::Fit.into(),
                sampling: ResizeSampling::Linear.into(),
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

    assert_eq!(compiled.plan.textures().len(), 4);
    assert_eq!(compiled.plan.buffers().len(), 4);
    assert_eq!(compiled.plan.programs().len(), 4);
    assert_eq!(compiled.plan.passes().len(), 4);
    assert_eq!(compiled.compiled_nodes.len(), 4);
    assert!(compiled.compiled_nodes.contains_key(&transform));
    assert!(compiled.compiled_nodes.contains_key(&crop));
    assert!(compiled.compiled_nodes.contains_key(&resize));
    assert_eq!(transformed.domain.storage_size, lumen_gpu::Size::new(8, 4));
    assert_eq!(resized.domain.storage_size, lumen_gpu::Size::new(8, 4));

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
    assert_eq!(unchanged.texture_upload_count(), 0);
    assert!(unchanged.buffer_upload_count() > 0);
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
    assert_eq!(bind(0).texture_upload_count(), 0);
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
    assert!(expected.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
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
    assert!(rebound.buffer_upload_count() > 0);
    let pixels = render_rgba8(&mut renderer, &composition, 1, &EmptyMediaStore);
    assert!(pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
    let mut reference = gpu_composition_renderer().unwrap();
    assert_eq!(
        pixels,
        render_rgba8(&mut reference, &composition, 1, &EmptyMediaStore)
    );
}

#[cfg(feature = "msdf")]
#[test]
fn text_msdf_coverage_matches_raster_for_plain_italic_and_variable_weight() {
    use lumen_engine::node::source::text::{TextFontStyle, TextRenderMode};
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    #[derive(Debug, Clone)]
    struct InterFont;
    impl lumen_engine::media::FontResolver for InterFont {
        fn id(&self) -> &str {
            "msdf-inter-coverage"
        }
        fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
            Ok(vec![
                include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf").to_vec(),
            ])
        }
    }
    impl MediaStore for InterFont {
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }
        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }
        fn font_resolver_id(&self, family: &str) -> Option<String> {
            (family == "Inter").then(|| "msdf-inter-coverage".to_string())
        }
        fn get_font_resolver(
            &self,
            family: &str,
        ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
            (family == "Inter")
                .then(|| Box::new(self.clone()) as Box<dyn lumen_engine::media::FontResolver>)
        }
    }
    for (family, style, weight) in [
        ("Roboto", TextFontStyle::Normal, 400),
        ("Roboto", TextFontStyle::Italic, 400),
        ("Inter", TextFontStyle::Normal, 250),
        ("Inter", TextFontStyle::Normal, 850),
    ] {
        for size in [24.0, 72.0, 144.0] {
            let mut composition = text_test_composition(TextParamsDelegate {
                content: Deferred::value("AV text 28".to_string()),
                font_family: Deferred::value(family.to_string()),
                font_style: style.into(),
                font_weight: Deferred::value(weight),
                font_size: Deferred::value(size),
                position: Deferred::value((12.25, 8.5)),
                ..Default::default()
            });
            renderer
                .compile_with_media(
                    &composition,
                    &InterFont,
                    lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                )
                .unwrap();
            let raster = render_rgba8(&mut renderer, &composition, 0, &InterFont);
            let NodeKind::Text(text) = composition.graph.nodes.get_mut(&NodeId::new(1)).unwrap()
            else {
                unreachable!()
            };
            text.params.render_mode = TextRenderMode::Msdf.into();
            renderer
                .compile_with_media(
                    &composition,
                    &InterFont,
                    lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                )
                .unwrap();
            let bound = renderer.bind_frame(&composition, 0, &InterFont).unwrap();
            let instances = renderer
                .compiled()
                .unwrap()
                .plan
                .buffers()
                .iter()
                .find(|b| b.label.as_deref() == Some("text:1:instances"))
                .unwrap()
                .id;
            let has_fields = bound.frame_update().uploads().iter().any(|upload| {
                let lumen_gpu::Upload::Buffer { id, data, .. } = upload else {
                    return false;
                };
                *id == instances
                    && data
                        .as_chunks::<{ std::mem::size_of::<lumen_text::GpuGlyphInstance>() }>()
                        .0
                        .iter()
                        .any(|bytes| {
                            bytemuck::pod_read_unaligned::<lumen_text::GpuGlyphInstance>(bytes).mode
                                == lumen_text::GLYPH_MODE_MSDF
                        })
            });
            assert!(
                has_fields,
                "{family} {style:?} {weight} {size}: no MSDF glyphs"
            );
            let msdf = render_rgba8(&mut renderer, &composition, 0, &InterFont);
            let (intersection, union) = raster
                .as_chunks::<4>()
                .0
                .iter()
                .zip(msdf.as_chunks::<4>().0.iter())
                .fold((0u64, 0u64), |(i, u), (a, b)| {
                    (i + u64::from(a[3].min(b[3])), u + u64::from(a[3].max(b[3])))
                });
            let iou = intersection as f64 / union as f64;
            eprintln!("{family} {style:?} weight={weight} size={size}: IoU={iou:.3}");
            let minimum = if size == 24.0 { 0.68 } else { 0.90 };
            assert!(
                iou > minimum,
                "{family} {style:?} weight={weight} size={size}: coverage IoU={iou:.3}"
            );
        }
    }
}

fn text_test_composition(params: TextParamsDelegate) -> Composition {
    let mut graph = Graph::new();
    graph.nodes.insert(
        NodeId::new(1),
        NodeKind::Text(Text {
            id: NodeId::new(1),
            params,
        }),
    );
    graph.nodes.insert(
        NodeId::new(2),
        NodeKind::MediaOutput(MediaOutput {
            id: NodeId::new(2),
            source: PortRef::new(NodeId::new(1), "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = 1024;
    composition.render_settings.height = 256;
    composition
}

#[test]
fn text_standalone_compile_bind_execute_requires_injected_atlas() {
    use lumen_engine::node::source::text::TextRenderMode;

    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let _gpu_lock = GpuTestLock::acquire();
    let mut gpu = match pollster::block_on(lumen_gpu::Renderer::new()) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("skipping GPU-backed lumen-engine test: {error:#}");
            return;
        }
    };
    for mode in [
        TextRenderMode::Raster,
        TextRenderMode::Msdf,
        TextRenderMode::Auto,
    ] {
        let validation = gpu
            .device
            .push_error_scope(lumen_gpu::wgpu::ErrorFilter::Validation);
        let composition = text_test_composition(TextParamsDelegate {
            content: Deferred::value("Standalone text".to_string()),
            font_size: Deferred::value(72.0),
            render_mode: mode.into(),
            ..Default::default()
        });
        let compiled = CompileContext::new(&composition).compile().unwrap();
        gpu.prepare_plan(&compiled.plan).unwrap();
        let bound = FrameBindContext::new(&composition, 0)
            .bind(&compiled)
            .unwrap();
        let error = gpu
            .execute(&compiled.plan, &bound.frame_update())
            .unwrap_err();
        assert!(error.to_string().contains("replace placeholder textures"));

        compiled.prepare_text_atlas(&mut gpu).unwrap();
        gpu.execute(&compiled.plan, &bound.frame_update()).unwrap();
        bound.mark_uploaded().unwrap();
        let pixels = read_rgba8(&gpu, compiled.output);
        assert!(
            pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0),
            "{mode:?}"
        );
        assert!(pollster::block_on(validation.pop()).is_none());
    }
}

#[test]
fn text_nodes_share_one_fixed_atlas_with_large_second_node() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut composition = text_test_composition(TextParamsDelegate {
        content: Deferred::value("Atlas W".to_string()),
        font_size: Deferred::value(72.0),
        ..Default::default()
    });
    composition.graph.nodes.insert(
        NodeId::new(3),
        NodeKind::Text(Text {
            id: NodeId::new(3),
            params: TextParamsDelegate {
                content: Deferred::value("Atlas M".to_string()),
                font_size: Deferred::value(72.0),
                position: Deferred::value((0.0, 100.0)),
                ..Default::default()
            },
        }),
    );
    composition.graph.nodes.insert(
        NodeId::new(4),
        NodeKind::Merge(Merge {
            id: NodeId::new(4),
            base: PortRef::new(NodeId::new(1), "output".to_string()),
            overlay: PortRef::new(NodeId::new(3), "output".to_string()),
            ..Default::default()
        }),
    );
    let NodeKind::MediaOutput(output) = composition.graph.nodes.get_mut(&NodeId::new(2)).unwrap()
    else {
        unreachable!()
    };
    output.source = PortRef::new(NodeId::new(4), "output".to_string());
    let initial = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    let atlas_resources: Vec<_> = renderer
        .compiled()
        .unwrap()
        .plan
        .textures()
        .iter()
        .filter(|t| t.label.as_deref() == Some("text:shared-atlas"))
        .collect();
    assert_eq!(atlas_resources.len(), 1);
    let atlas = renderer
        .gpu_renderer()
        .texture_arc(atlas_resources[0].id)
        .unwrap();
    renderer.release_resources();
    let rebound = renderer
        .bind_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    assert_eq!(
        rebound.texture_upload_count(),
        0,
        "recompilation retains atlas residency"
    );
    let atlas_id = renderer
        .compiled()
        .unwrap()
        .plan
        .textures()
        .iter()
        .find(|t| t.label.as_deref() == Some("text:shared-atlas"))
        .unwrap()
        .id;
    assert!(Arc::ptr_eq(
        &atlas,
        &renderer.gpu_renderer().texture_arc(atlas_id).unwrap()
    ));
    assert_eq!(
        initial,
        render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore)
    );
    let NodeKind::Text(text) = composition.graph.nodes.get_mut(&NodeId::new(3)).unwrap() else {
        unreachable!()
    };
    text.params.content = Deferred::value("W".to_string());
    text.params.font_size = Deferred::value(3800.0);
    renderer.release_resources();
    let bound = renderer
        .bind_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    renderer.upload_bound_frame(&bound).unwrap();
    let atlas_id = renderer
        .compiled()
        .unwrap()
        .plan
        .textures()
        .iter()
        .find(|t| t.label.as_deref() == Some("text:shared-atlas"))
        .unwrap()
        .id;
    assert!(Arc::ptr_eq(
        &atlas,
        &renderer.gpu_renderer().texture_arc(atlas_id).unwrap()
    ));
    let instances_id = renderer
        .compiled()
        .unwrap()
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some("text:3:instances"))
        .unwrap()
        .id;
    assert!(bound.frame_update().uploads().iter().any(|upload| {
        let lumen_gpu::Upload::Buffer { id, data, .. } = upload else {
            return false;
        };
        *id == instances_id
            && data
                .as_chunks::<{ std::mem::size_of::<lumen_text::GpuGlyphInstance>() }>()
                .0
                .iter()
                .any(|bytes| {
                    let glyph = bytemuck::pod_read_unaligned::<lumen_text::GpuGlyphInstance>(bytes);
                    glyph.rect[2] > 2500.0 && glyph.rect[3] > 2500.0
                })
    }));
    let enlarged = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    assert!(enlarged.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
    let mut reference = gpu_composition_renderer().unwrap();
    assert_eq!(
        enlarged,
        render_rgba8(&mut reference, &composition, 0, &EmptyMediaStore)
    );
    renderer.release_text_atlas();
    assert!(renderer.compiled().is_none());
    let rebound = renderer
        .bind_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    assert!(rebound.texture_upload_count() > 0);
    renderer.upload_bound_frame(&rebound).unwrap();
    assert_eq!(
        enlarged,
        render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore)
    );
}

#[test]
fn text_atlas_overflow_does_not_thrash_on_repeated_binds() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    const CONTENT: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut composition = text_test_composition(TextParamsDelegate {
        content: Deferred::value(CONTENT.to_string()),
        font_size: Deferred::value(1800.0),
        ..Default::default()
    });
    let first = renderer
        .bind_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    assert!(first.texture_upload_count() > 0);
    assert!(
        first.texture_upload_count() < CONTENT.chars().count(),
        "the layout must overflow"
    );
    renderer.upload_bound_frame(&first).unwrap();
    for frame in 1..4 {
        let repeated = renderer
            .bind_frame(&composition, frame, &EmptyMediaStore)
            .unwrap();
        assert_eq!(repeated.texture_upload_count(), 0);
        renderer.upload_bound_frame(&repeated).unwrap();
    }
    let NodeKind::Text(text) = composition.graph.nodes.get_mut(&NodeId::new(1)).unwrap() else {
        unreachable!()
    };
    text.params.content = Deferred::value("W".to_string());
    text.params.font_size = Deferred::value(20000.0);
    renderer.release_resources();
    for frame in 4..7 {
        let oversized = renderer
            .bind_frame(&composition, frame, &EmptyMediaStore)
            .unwrap();
        // With distance fields, the raster glyph is too big for the atlas, so one
        // field is drawn instead and reused. Without them the glyph is dropped.
        assert_eq!(
            oversized.texture_upload_count(),
            usize::from(cfg!(feature = "msdf") && frame == 4)
        );
        renderer.upload_bound_frame(&oversized).unwrap();
    }
    let NodeKind::Text(text) = composition.graph.nodes.get_mut(&NodeId::new(1)).unwrap() else {
        unreachable!()
    };
    text.params.font_size = Deferred::value(72.0);
    renderer.release_resources();
    let recovered = renderer
        .bind_frame(&composition, 7, &EmptyMediaStore)
        .unwrap();
    assert!(recovered.texture_upload_count() > 0);
}

#[test]
fn text_transparent_gradient_midpoint_is_premultiplied() {
    use lumen_engine::node::{
        source::text::TextRenderMode,
        vector::paint::{GradientPaint, GradientStop, GradientUnits},
    };
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        for supersample in [false, true] {
            let composition = text_test_composition(TextParamsDelegate {
                content: Deferred::value("M".to_string()),
                font_size: Deferred::value(180.0),
                render_mode: mode.into(),
                paint_supersample: Deferred::value(supersample),
                color: Paint::Gradient(GradientPaint {
                    units: GradientUnits::UserSpace,
                    start: [-1000000.0, 0.0],
                    end: [1000000.0, 0.0],
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: [255, 0, 0, 0],
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
            });
            renderer.release_resources();
            let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] >= 126)
            );
            for pixel in pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[3] >= 126)
            {
                assert!(
                    pixel[0] <= 1
                        && pixel[1] <= 1
                        && pixel[2].abs_diff(128) <= 2
                        && pixel[3].abs_diff(128) <= 2,
                    "{mode:?}, supersample={supersample}: {pixel:?}"
                );
            }
        }
    }
}

#[test]
fn text_measurement_loads_time_remapped_family_on_first_bind() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    #[derive(Debug, Clone)]
    struct FontStore {
        data: Vec<u8>,
        id: &'static str,
        resolver_calls: Arc<AtomicUsize>,
    }

    impl lumen_engine::media::FontResolver for FontStore {
        fn id(&self) -> &str {
            self.id
        }

        fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
            Ok(vec![self.data.clone()])
        }
    }

    impl MediaStore for FontStore {
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }

        fn font_resolver_id(&self, family: &str) -> Option<String> {
            (family == "RemapX").then(|| self.id.to_string())
        }

        fn get_font_resolver(
            &self,
            family: &str,
        ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
            (family == "RemapX").then(|| {
                self.resolver_calls.fetch_add(1, Ordering::Relaxed);
                Box::new(self.clone()) as _
            })
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
        .as_chunks::<16>()
        .0
        .iter()
        .find(|table| &table[..4] == b"head")
        .unwrap();
    let units_offset = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize + 18;
    let units = u16::from_be_bytes(font[units_offset..units_offset + 2].try_into().unwrap());
    font[units_offset..units_offset + 2].copy_from_slice(&(units * 2).to_be_bytes());
    let mut store = FontStore {
        data: font.clone(),
        id: "remapped-text-test:1",
        resolver_calls: Arc::new(AtomicUsize::new(0)),
    };
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
    assert_eq!(store.resolver_calls.load(Ordering::Relaxed), 1);

    store.id = "remapped-text-test:2";
    let replaced = FrameBindContext::with_media(&composition, 0, &store)
        .bind(&compiled)
        .unwrap();
    assert_eq!(first_rect(&first), first_rect(&replaced));
    assert_eq!(store.resolver_calls.load(Ordering::Relaxed), 2);
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
        .as_chunks::<{ std::mem::size_of::<lumen_text::GpuGlyphInstance>() }>()
        .0
        .iter()
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
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
        "subpixel path should exercise antialiased fill coverage"
    );
}

#[test]
fn polygon_shape_renders_its_points_instead_of_a_rectangle() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shape = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            params: ShapeParamsDelegate {
                geometry_kind: ShapeGeometryKind::Polygon.into(),
                width: Deferred::value(12),
                height: Deferred::value(12),
                polygon_points: Deferred::value("0,0; 12,0; 0,12".to_string()),
                position: Deferred::value((2.0, 2.0)),
                fill_paint: Paint::solid([255, 0, 0, 255]).into(),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(shape, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = 16;
    composition.render_settings.height = 16;

    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    let pixel = |x: usize, y: usize| &pixels[(y * 16 + x) * 4..][..4];

    for (x, y) in [(3, 3), (10, 3), (3, 10), (6, 6)] {
        assert_eq!(
            pixel(x, y),
            [255, 0, 0, 255],
            "({x}, {y}) is inside the triangle"
        );
    }
    for (x, y) in [(13, 13), (12, 8), (8, 12), (0, 0), (15, 15)] {
        assert_eq!(
            pixel(x, y),
            [0, 0, 0, 0],
            "({x}, {y}) is outside the triangle"
        );
    }
}

fn render_shape_rgba8(params: ShapeParamsDelegate, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut renderer = gpu_composition_renderer()?;
    let shape = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(shape, NodeKind::Shape(Shape { id: shape, params }));
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(shape, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = width;
    composition.render_settings.height = height;
    Some(render_rgba8(
        &mut renderer,
        &composition,
        0,
        &EmptyMediaStore,
    ))
}

#[test]
fn rounded_rectangle_shape_clears_corners_and_fills_centre() {
    let Some(pixels) = render_shape_rgba8(
        ShapeParamsDelegate {
            width: Deferred::value(16),
            height: Deferred::value(16),
            border_radius: Deferred::value(8.0),
            position: Deferred::value((2.0, 2.0)),
            fill_paint: Paint::solid([255, 0, 0, 255]).into(),
            ..Default::default()
        },
        20,
        20,
    ) else {
        return;
    };
    let pixel = |x: usize, y: usize| &pixels[(y * 20 + x) * 4..][..4];

    for (x, y) in [(10, 10), (10, 3), (3, 10), (16, 10), (10, 16)] {
        assert_eq!(pixel(x, y), [255, 0, 0, 255], "({x}, {y}) is inside");
    }
    for (x, y) in [(2, 2), (17, 2), (2, 17), (17, 17), (0, 0), (19, 19)] {
        assert_eq!(pixel(x, y), [0, 0, 0, 0], "({x}, {y}) is outside");
    }
}

#[test]
fn ellipse_shape_fills_its_inscribed_ellipse() {
    let Some(pixels) = render_shape_rgba8(
        ShapeParamsDelegate {
            geometry_kind: ShapeGeometryKind::Ellipse.into(),
            width: Deferred::value(20),
            height: Deferred::value(10),
            position: Deferred::value((2.0, 4.0)),
            fill_paint: Paint::solid([0, 255, 0, 255]).into(),
            ..Default::default()
        },
        24,
        18,
    ) else {
        return;
    };
    let pixel = |x: usize, y: usize| &pixels[(y * 24 + x) * 4..][..4];

    for (x, y) in [(12, 9), (4, 9), (19, 9), (12, 5), (12, 12)] {
        assert_eq!(pixel(x, y), [0, 255, 0, 255], "({x}, {y}) is inside");
    }
    for (x, y) in [(2, 4), (21, 4), (2, 13), (21, 13), (12, 2), (12, 15)] {
        assert_eq!(pixel(x, y), [0, 0, 0, 0], "({x}, {y}) is outside");
    }
}

#[test]
fn polygon_shape_with_invalid_points_draws_nothing_without_stale_geometry() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shape = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            params: ShapeParamsDelegate {
                geometry_kind: ShapeGeometryKind::Polygon.into(),
                polygon_points: Deferred::Expr(
                    Expression::parse("if(frame == 1, \"\", \"0,0; 12,0; 0,12\")").unwrap(),
                ),
                position: Deferred::value((2.0, 2.0)),
                fill_paint: Paint::solid([255, 0, 0, 255]).into(),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(shape, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = 16;
    composition.render_settings.height = 16;

    for (frame, expected) in [
        (0, [255, 0, 0, 255]),
        (1, [0, 0, 0, 0]),
        (2, [255, 0, 0, 255]),
    ] {
        let pixels = render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore);
        assert_eq!(
            &pixels[(3 * 16 + 3) * 4..][..4],
            expected,
            "frame {frame} pixel inside the triangle"
        );
    }
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
                        x: Deferred::value(0),
                        y: Deferred::value(0),
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
    assert!(
        upscaled
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == 255)
    );
}

/// Alpha channel per pixel of an 8x4 render, as rows of 8 characters.
fn alpha_rows(pixels: &[u8]) -> Vec<String> {
    pixels
        .as_chunks::<4>()
        .0
        .chunks(8)
        .map(|row| {
            row.iter()
                .map(|pixel| if pixel[3] == 255 { '#' } else { '.' })
                .collect()
        })
        .collect()
}

fn single_node_composition(node: NodeKind, source_width: u32, source_height: u32) -> Composition {
    let background = NodeId::new(1);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph.nodes.insert(
        background,
        NodeKind::Background(Background {
            id: background,
            params: BackgroundParamsDelegate {
                paint: lumen_engine::node::vector::paint::PaintDelegate::from(
                    lumen_engine::node::vector::paint::Paint::solid([255, 0, 0, 255]),
                ),
                width: Deferred::value(source_width),
                height: Deferred::value(source_height),
                paint_supersample: Deferred::value(true),
            },
        }),
    );
    let node_id = NodeId::new(2);
    graph.nodes.insert(node_id, node);
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(node_id, "output".to_string()),
        }),
    );
    test_composition(graph)
}

#[test]
fn crop_keeps_its_region_in_place_and_follows_animated_size() {
    let crop = |width: Deferred<i64>| {
        single_node_composition(
            NodeKind::Crop(Crop {
                id: NodeId::new(2),
                params: CropParamsDelegate {
                    x: Deferred::value(2),
                    y: Deferred::value(1),
                    width,
                    height: Deferred::value(2),
                },
                source: PortRef::new(NodeId::new(1), "output".to_string()),
            }),
            8,
            4,
        )
    };
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };

    let fixed = crop(Deferred::value(3));
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &fixed, 0, &EmptyMediaStore)),
        ["........", "..###...", "..###...", "........"]
    );

    let animated = crop(Deferred::Expr(
        lumen_engine::expr::Expression::parse("frame").unwrap(),
    ));
    // A fresh renderer: plans are cached per renderer by node selections only, so animated
    // sizes must work without a recompile across frames, not across compositions.
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &animated, 2, &EmptyMediaStore))[1],
        "..##...."
    );
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &animated, 5, &EmptyMediaStore))[1],
        "..#####."
    );
}

#[test]
fn resize_places_its_output_and_follows_animated_size() {
    let resize = |width: Deferred<i64>| {
        single_node_composition(
            NodeKind::Resize(Resize {
                id: NodeId::new(2),
                params: ResizeParamsDelegate {
                    x: Deferred::value(3),
                    y: Deferred::value(1),
                    width,
                    height: Deferred::value(2),
                    mode: ResizeMode::Stretch.into(),
                    sampling: ResizeSampling::Nearest.into(),
                },
                source: PortRef::new(NodeId::new(1), "output".to_string()),
            }),
            4,
            2,
        )
    };
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };

    let fixed = resize(Deferred::value(2));
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &fixed, 0, &EmptyMediaStore)),
        ["........", "...##...", "...##...", "........"]
    );

    let animated = resize(Deferred::Expr(
        lumen_engine::expr::Expression::parse("frame").unwrap(),
    ));
    // A fresh renderer: plans are cached per renderer by node selections only, so animated
    // sizes must work without a recompile across frames, not across compositions.
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &animated, 1, &EmptyMediaStore))[1],
        "...#...."
    );
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &animated, 4, &EmptyMediaStore))[1],
        "...####."
    );
}

fn crop_then_resize(
    crop: [Deferred<i64>; 4],
    resize: [Deferred<i64>; 4],
    mode: ResizeMode,
) -> Composition {
    // `single_node_composition` already owns node 3 as the media output; it is repointed below.
    let (background, crop_id, resize_id, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(5),
        NodeId::new(3),
    );
    let [x, y, width, height] = crop;
    let mut composition = single_node_composition(
        NodeKind::Crop(Crop {
            id: crop_id,
            params: CropParamsDelegate {
                x,
                y,
                width,
                height,
            },
            source: PortRef::new(background, "output".to_string()),
        }),
        8,
        4,
    );
    let [x, y, width, height] = resize;
    composition.graph.nodes.insert(
        resize_id,
        NodeKind::Resize(Resize {
            id: resize_id,
            params: ResizeParamsDelegate {
                x,
                y,
                width,
                height,
                mode: mode.into(),
                sampling: ResizeSampling::Linear.into(),
            },
            source: PortRef::new(crop_id, "output".to_string()),
        }),
    );
    composition.graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(resize_id, "output".to_string()),
        }),
    );
    composition
}

fn values(values: [i64; 4]) -> [Deferred<i64>; 4] {
    values.map(Deferred::value)
}

#[test]
fn resize_uses_the_cropped_region_as_its_source_bounds() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let fit = crop_then_resize(values([2, 0, 4, 4]), values([0, 0, 4, 4]), ResizeMode::Fit);
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &fit, 0, &EmptyMediaStore)),
        ["####....", "####....", "####....", "####...."]
    );

    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let fill = crop_then_resize(values([0, 0, 2, 4]), values([0, 0, 2, 4]), ResizeMode::Fill);
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &fill, 0, &EmptyMediaStore)),
        ["##......", "##......", "##......", "##......"]
    );
}

#[test]
fn resize_follows_an_animated_crop_region() {
    let frame = || Deferred::Expr(lumen_engine::expr::Expression::parse("frame").unwrap());
    let [x, y, _, height] = values([0, 0, 0, 4]);
    let composition = crop_then_resize(
        [x, y, frame(), height],
        values([0, 0, 4, 4]),
        ResizeMode::Fit,
    );
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // A 2x4 region is centred in the 4x4 box; a 4x4 region fills it.
    assert_eq!(
        alpha_rows(&render_rgba8(
            &mut renderer,
            &composition,
            2,
            &EmptyMediaStore
        ))[0],
        ".##....."
    );
    assert_eq!(
        alpha_rows(&render_rgba8(
            &mut renderer,
            &composition,
            4,
            &EmptyMediaStore
        ))[0],
        "####...."
    );
}

#[test]
fn crop_and_resize_reject_values_the_gpu_cannot_represent() {
    // The renderer caches compiled plans, so each composition gets its own.
    let error_for = |crop: [i64; 4], resize: [i64; 4]| {
        let mut renderer = gpu_composition_renderer()?;
        let composition = crop_then_resize(values(crop), values(resize), ResizeMode::Stretch);
        Some(
            renderer
                .render_frame(&composition, 0, &EmptyMediaStore)
                .map(|_| ())
                .unwrap_err()
                .to_string(),
        )
    };
    let valid = [0, 0, 4, 4];
    for (index, property) in ["x", "y", "width", "height"].into_iter().enumerate() {
        let mut invalid = valid;
        invalid[index] = 1 << 32;
        let Some(crop_error) = error_for(invalid, valid) else {
            return;
        };
        let resize_error = error_for(valid, invalid).unwrap();
        for message in [crop_error, resize_error] {
            assert!(message.contains(&format!("`{property}`")), "{message}");
        }
    }
    let Some(message) = error_for([i64::from(i32::MIN) - 1, 0, 4, 4], valid) else {
        return;
    };
    assert!(message.contains("`x`"), "{message}");
}

#[test]
fn crop_accepts_sizes_beyond_i32() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let crop = single_node_composition(
        NodeKind::Crop(Crop {
            id: NodeId::new(2),
            params: CropParamsDelegate {
                x: Deferred::value(0),
                y: Deferred::value(0),
                width: Deferred::value(1 << 31),
                height: Deferred::value(i64::from(u32::MAX)),
            },
            source: PortRef::new(NodeId::new(1), "output".to_string()),
        }),
        8,
        4,
    );
    assert_eq!(
        alpha_rows(&render_rgba8(&mut renderer, &crop, 0, &EmptyMediaStore)),
        ["########"; 4]
    );
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
        for pixel in pixels.as_chunks::<4>().0 {
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
    for pixel in pixels.as_chunks::<4>().0 {
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

fn gpu_composition_renderer() -> Option<TestRenderer> {
    let lock = GpuTestLock::acquire();
    match pollster::block_on(lumen_engine::gpu::GpuCompositionRenderer::new()) {
        Ok(renderer) => Some(TestRenderer {
            renderer,
            _lock: lock,
        }),
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
    media: &impl MediaStore,
) -> Vec<u8> {
    let raster = renderer.render_frame(composition, frame, media).unwrap();
    read_rgba8(renderer.gpu_renderer(), raster)
}

fn read_rgba8(gpu: &lumen_gpu::Renderer, raster: lumen_engine::gpu::RasterHandle) -> Vec<u8> {
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

#[test]
fn text_spacing_and_variable_weight_change_output_smoothly() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    #[derive(Debug, Clone)]
    struct InterFont;
    impl lumen_engine::media::FontResolver for InterFont {
        fn id(&self) -> &str {
            "msdf-inter-coverage"
        }
        fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
            Ok(vec![
                include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf").to_vec(),
            ])
        }
    }
    impl MediaStore for InterFont {
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }
        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }
        fn font_resolver_id(&self, family: &str) -> Option<String> {
            (family == "Inter").then(|| "msdf-inter-coverage".to_string())
        }
        fn get_font_resolver(
            &self,
            family: &str,
        ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
            (family == "Inter")
                .then(|| Box::new(self.clone()) as Box<dyn lumen_engine::media::FontResolver>)
        }
    }
    let base = || TextParamsDelegate {
        content: Deferred::value("Hi\nHo".to_string()),
        font_family: Deferred::value("Inter".to_string()),
        font_size: Deferred::value(40.0),
        position: Deferred::value((20.0, 10.0)),
        ..Default::default()
    };
    let mut render = |params: TextParamsDelegate, frame: u32| {
        let composition = text_test_composition(params);
        renderer
            .compile_with_media(
                &composition,
                &InterFont,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
        render_rgba8(&mut renderer, &composition, frame, &InterFont)
    };
    let width = 1024_usize;
    // Rightmost and bottommost inked pixel as (x, y), and total ink.
    let extent = |pixels: &[u8]| {
        let (mut right, mut bottom, mut ink) = (0, 0, 0_u64);
        for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] > 0 {
                right = right.max(index % width);
                bottom = bottom.max(index / width);
                ink += u64::from(pixel[3]);
            }
        }
        (right, bottom, ink)
    };

    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        let with = |letter_spacing, line_height| TextParamsDelegate {
            render_mode: mode.into(),
            letter_spacing: Deferred::value(letter_spacing),
            line_height: Deferred::value(line_height),
            ..base()
        };
        let auto = render(with(0.0, 0.0), 0);
        let (auto_right, auto_bottom, _) = extent(&auto);
        assert!(auto_right > 20, "{mode:?}: nothing rendered");

        // An explicit line height equal to the automatic one changes nothing.
        let explicit = render(with(0.0, f64::from(40.0_f32 * 1.2)), 0);
        assert!(
            auto == explicit,
            "{mode:?}: explicit auto line height differs"
        );

        let (spaced_right, spaced_bottom, _) = extent(&render(with(20.0, 0.0), 0));
        assert!(
            spaced_right >= auto_right + 15,
            "{mode:?}: letter spacing did not widen ({spaced_right} vs {auto_right})"
        );
        assert_eq!(
            spaced_bottom, auto_bottom,
            "{mode:?}: letter spacing moved lines"
        );

        let (_, tall_bottom, _) = extent(&render(with(0.0, 100.0), 0));
        assert!(
            tall_bottom >= auto_bottom + 40,
            "{mode:?}: line height did not apply"
        );

        let (tight_right, _, _) = extent(&render(with(-4.0, 0.0), 0));
        assert!(
            tight_right < auto_right,
            "{mode:?}: negative spacing did not tighten"
        );
    }

    // Weights 400, 450 and 500 are distinct, and an animated weight grows the
    // glyph ink at every step.
    let at_weight = |weight| TextParamsDelegate {
        font_size: Deferred::value(96.0),
        font_weight: Deferred::value(weight),
        ..base()
    };
    let (w400, w450, w500) = (
        render(at_weight(400), 0),
        render(at_weight(450), 0),
        render(at_weight(500), 0),
    );
    assert!(w450 != w400 && w450 != w500 && w400 != w500);
    let animated = TextParamsDelegate {
        font_weight: Deferred::Expr(
            lumen_engine::expr::Expression::parse("400 + frame * 10").unwrap(),
        ),
        ..at_weight(400)
    };
    let mut previous = 0;
    for frame in 0..=10 {
        let (_, _, ink) = extent(&render(animated.clone(), frame));
        assert!(
            ink > previous,
            "frame {frame}: ink {ink} did not grow from {previous}"
        );
        previous = ink;
    }
    assert_eq!(render(animated.clone(), 5), w450);
}

#[cfg(feature = "msdf")]
#[test]
fn text_glyphs_larger_than_the_atlas_still_draw() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // At 6000 px the stem of an "I" is taller than the 4096 px atlas.
    let mut composition = text_test_composition(TextParamsDelegate {
        content: Deferred::value("I".to_string()),
        font_size: Deferred::value(6000.0),
        position: Deferred::value((-700.0, -2000.0)),
        ..Default::default()
    });
    composition.render_settings.width = 256;
    composition.render_settings.height = 256;
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    let inked = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] == 255)
        .count();
    assert!(inked > 256 * 256 / 2, "only {inked} pixels inked");
}

#[cfg(feature = "msdf")]
#[test]
fn text_msdf_corners_stay_within_a_pixel_of_raster_at_1000px() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (width, height) = (1500_usize, 1100_usize);
    let mut render = |mode: TextRenderMode| {
        let mut composition = text_test_composition(TextParamsDelegate {
            content: Deferred::value("Ra".to_string()),
            font_size: Deferred::value(1000.0),
            position: Deferred::value((20.0, -180.0)),
            render_mode: mode.into(),
            ..Default::default()
        });
        composition.render_settings.width = width as u32;
        composition.render_settings.height = height as u32;
        renderer
            .compile_with_media(
                &composition,
                &EmptyMediaStore,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
        let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>()
    };
    let raster = render(TextRenderMode::Raster);
    let msdf = render(TextRenderMode::Msdf);
    assert!(raster.iter().filter(|alpha| **alpha == 255).count() > 50_000);

    // Fields with too few texels per em cut facets of several pixels into
    // sharp corners, so the field must stay inside the raster outline dilated
    // and eroded by one pixel.
    let mut outliers = 0;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let (mut low, mut high) = (255_u8, 0_u8);
            for ny in y - 1..=y + 1 {
                for nx in x - 1..=x + 1 {
                    low = low.min(raster[ny * width + nx]);
                    high = high.max(raster[ny * width + nx]);
                }
            }
            let value = msdf[y * width + x];
            outliers += usize::from(low.saturating_sub(value).max(value.saturating_sub(high)) > 26);
        }
    }
    assert!(outliers <= 16, "{outliers} pixels off the raster outline");
}
