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
            boolean::{Boolean, BooleanOperation, BooleanParamsDelegate},
            merge::{MaskMode, Merge, MergeParamsDelegate},
            raster_multimerge::{RasterMultiMerge, RasterMultiMergeParamsDelegate},
            switch::{Switch, SwitchParamsDelegate},
        },
        media_output::MediaOutput,
        processing::{
            alpha_premultiply::{AlphaPremultiply, AlphaPremultiplyParamsDelegate},
            blur::{Blur, BlurParamsDelegate},
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
    assert_eq!(bound.frame_update().uploads().len(), 3);
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

const MASK_CANVAS_WIDTH: usize = 24;
// Tall enough that the middle row is unaffected by the transparent area beyond the top and bottom.
const MASK_CANVAS_HEIGHT: usize = 64;
const MASK_ROW: usize = MASK_CANVAS_HEIGHT / 2;

fn sized_solid(id: NodeId, color: [u8; 4], width: usize) -> NodeKind {
    sized_solid_rect(id, color, width, MASK_CANVAS_HEIGHT)
}

fn sized_solid_rect(id: NodeId, color: [u8; 4], width: usize, height: usize) -> NodeKind {
    NodeKind::Background(Background {
        id,
        params: BackgroundParamsDelegate {
            paint: Paint::solid(color).into(),
            width: Deferred::value(width as u32),
            height: Deferred::value(height as u32),
            paint_supersample: Deferred::value(true),
        },
    })
}

fn wide_solid(id: NodeId, color: [u8; 4]) -> NodeKind {
    sized_solid(id, color, MASK_CANVAS_WIDTH)
}

/// A white-filled rectangle spanning the canvas height with half-covered pixels on both sides.
fn edged_rectangle(id: NodeId, x: f64, width: i64, color: [u8; 4]) -> NodeKind {
    NodeKind::Shape(Shape {
        id,
        params: ShapeParamsDelegate {
            width: Deferred::value(width),
            height: Deferred::value(MASK_CANVAS_HEIGHT as i64),
            position: Deferred::value((x, 0.0)),
            fill_paint: Paint::solid(color).into(),
            ..Default::default()
        },
    })
}

/// Renders the red channel of white merged over black through `mask`, so the result is the matte.
fn render_matte(
    mask_nodes: impl FnOnce(&mut Graph) -> NodeId,
    params: MergeParamsDelegate,
) -> Vec<u8> {
    render_matte_on(MASK_CANVAS_WIDTH, mask_nodes, params)
}

fn render_matte_on(
    canvas_width: usize,
    mask_nodes: impl FnOnce(&mut Graph) -> NodeId,
    params: MergeParamsDelegate,
) -> Vec<u8> {
    let (base, overlay, merge, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
    );
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(base, sized_solid(base, [0, 0, 0, 255], canvas_width));
    graph.nodes.insert(
        overlay,
        sized_solid(overlay, [255, 255, 255, 255], canvas_width),
    );
    let mask = mask_nodes(&mut graph);
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            params,
            base: PortRef::new(base, "output".to_string()),
            overlay: PortRef::new(overlay, "output".to_string()),
            mask: PortRef::new(mask, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(merge, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = canvas_width as u32;
    composition.render_settings.height = MASK_CANVAS_HEIGHT as u32;
    let Some(mut renderer) = gpu_composition_renderer() else {
        return Vec::new();
    };
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    // The middle row of the red channel.
    (0..canvas_width)
        .map(|x| pixels[(MASK_ROW * canvas_width + x) * 4])
        .collect()
}

fn assert_matte(actual: &[u8], expected: &[(usize, u8)]) {
    if actual.is_empty() {
        return;
    }
    for &(x, value) in expected {
        assert!(
            actual[x].abs_diff(value) <= 2,
            "column {x}: expected {value}, got {} in {actual:?}",
            actual[x]
        );
    }
}

#[test]
fn merge_alpha_mask_keeps_antialiased_edges() {
    // Covers columns 8 (half), 9..=15 (full) and 16 (half).
    let matte = render_matte(
        |graph| {
            let mask = NodeId::new(5);
            graph
                .nodes
                .insert(mask, edged_rectangle(mask, 8.5, 8, [255, 255, 255, 255]));
            mask
        },
        MergeParamsDelegate::default(),
    );
    assert_matte(
        &matte,
        &[
            (0, 0),
            (7, 0),
            (8, 128),
            (9, 255),
            (15, 255),
            (16, 128),
            (17, 0),
        ],
    );
}

#[test]
fn merge_luma_mask_uses_rec709_over_black() {
    let luma = || MergeParamsDelegate {
        mask_mode: MaskMode::Luma.into(),
        ..Default::default()
    };

    // An opaque red edge-antialiased mask: alpha mode would give 255 and 128 here.
    let matte = render_matte(
        |graph| {
            let mask = NodeId::new(5);
            graph
                .nodes
                .insert(mask, edged_rectangle(mask, 8.5, 8, [255, 0, 0, 255]));
            mask
        },
        luma(),
    );
    assert_matte(
        &matte,
        &[(7, 0), (8, 27), (9, 54), (15, 54), (16, 27), (17, 0)],
    );

    let matte = render_matte(
        |graph| {
            let mask = NodeId::new(5);
            graph.nodes.insert(mask, wide_solid(mask, [0, 255, 0, 255]));
            mask
        },
        luma(),
    );
    assert_matte(&matte, &[(0, 182), (12, 182)]);

    // A straight-alpha mask is premultiplied before taking luma: white at 50% alpha is 50%.
    let matte = render_matte(
        |graph| {
            let (solid_id, mask) = (NodeId::new(5), NodeId::new(6));
            graph
                .nodes
                .insert(solid_id, wide_solid(solid_id, [255, 255, 255, 128]));
            graph.nodes.insert(
                mask,
                NodeKind::AlphaPremultiply(AlphaPremultiply {
                    id: mask,
                    params: AlphaPremultiplyParamsDelegate {
                        mode: Deferred::value("unpremultiply".to_string()),
                    },
                    source: PortRef::new(solid_id, "output".to_string()),
                }),
            );
            mask
        },
        luma(),
    );
    assert_matte(&matte, &[(0, 128), (12, 128)]);
}

#[test]
fn merge_invert_flips_alpha_and_luma_masks() {
    let rectangle = |graph: &mut Graph| {
        let mask = NodeId::new(5);
        graph
            .nodes
            .insert(mask, edged_rectangle(mask, 8.5, 8, [255, 255, 255, 255]));
        mask
    };
    for mask_mode in [MaskMode::Alpha, MaskMode::Luma] {
        let matte = render_matte(
            rectangle,
            MergeParamsDelegate {
                mask_mode: mask_mode.into(),
                invert: Deferred::value(true),
                ..Default::default()
            },
        );
        assert_matte(
            &matte,
            &[
                (0, 255),
                (7, 255),
                (8, 127),
                (9, 0),
                (15, 0),
                (16, 127),
                (17, 255),
            ],
        );
    }
}

#[test]
fn merge_feather_softens_mask_edges_and_preserves_coverage() {
    let rectangle = |graph: &mut Graph| {
        let mask = NodeId::new(5);
        graph
            .nodes
            .insert(mask, edged_rectangle(mask, 8.5, 8, [255, 255, 255, 255]));
        mask
    };
    let sharp = render_matte(rectangle, MergeParamsDelegate::default());
    let soft = render_matte(
        rectangle,
        MergeParamsDelegate {
            feather: Deferred::value(2.0),
            ..Default::default()
        },
    );
    if soft.is_empty() {
        return;
    }

    assert_matte(&soft, &[(0, 0), (2, 0), (12, 255)]);
    // The sharp edge ramps up over several pixels instead of one.
    assert!(
        soft[7] > 0 && soft[8] > 0,
        "feather spreads the matte outside the shape: {soft:?}"
    );
    assert!(
        soft[9] < 255,
        "feather spreads the matte inside the shape: {soft:?}"
    );
    assert!(
        soft[6..=11].windows(2).all(|pair| pair[0] <= pair[1]),
        "the feathered edge rises monotonically: {soft:?}"
    );
    // Averaging redistributes coverage without adding or removing any.
    let total = |matte: &[u8]| matte.iter().map(|&value| i32::from(value)).sum::<i32>();
    let (sharp_total, soft_total) = (total(&sharp), total(&soft));
    assert!(
        (sharp_total - soft_total).abs() * 20 <= sharp_total,
        "total coverage changed from {sharp_total} to {soft_total}"
    );

    // Negative values behave like no feather, and huge values stay bounded and still render.
    let negative = render_matte(
        rectangle,
        MergeParamsDelegate {
            feather: Deferred::value(-3.0),
            ..Default::default()
        },
    );
    assert_eq!(negative, sharp);
    let huge = render_matte(
        rectangle,
        MergeParamsDelegate {
            feather: Deferred::value(1.0e9),
            ..Default::default()
        },
    );
    assert_eq!(huge.len(), MASK_CANVAS_WIDTH);
}

#[test]
fn merge_feather_spreads_outside_a_smaller_mask() {
    // The mask texture is half the canvas wide, so its right edge is a texture edge.
    let mask_nodes = |graph: &mut Graph| {
        let mask = NodeId::new(5);
        graph
            .nodes
            .insert(mask, sized_solid(mask, [255, 255, 255, 255], 12));
        mask
    };
    let sharp = render_matte(mask_nodes, MergeParamsDelegate::default());
    assert_matte(&sharp, &[(11, 255), (12, 0)]);

    let soft = render_matte(
        mask_nodes,
        MergeParamsDelegate {
            feather: Deferred::value(2.0),
            ..Default::default()
        },
    );
    if soft.is_empty() {
        return;
    }
    assert!(
        soft[12] > 40 && soft[13] > 0,
        "feather spreads past the mask texture: {soft:?}"
    );
    assert!(soft[11] < 230, "feather softens the inside: {soft:?}");
    assert!(
        soft[6..=16].windows(2).all(|pair| pair[0] >= pair[1]),
        "the feathered edge falls monotonically: {soft:?}"
    );
}

/// Compiles a merge of two canvas-sized solids through a solid mask of the given size.
fn compile_feathered_merge(
    mask_size: (usize, usize),
    feather: Deferred<f64>,
) -> lumen_engine::gpu::CompiledComposition {
    let (base, overlay, mask, merge, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(5),
        NodeId::new(3),
        NodeId::new(4),
    );
    let mut graph = Graph::new();
    graph.nodes.insert(base, wide_solid(base, [0, 0, 0, 255]));
    graph
        .nodes
        .insert(overlay, wide_solid(overlay, [255, 255, 255, 255]));
    graph.nodes.insert(
        mask,
        sized_solid_rect(mask, [255, 255, 255, 255], mask_size.0, mask_size.1),
    );
    graph.nodes.insert(
        merge,
        NodeKind::Merge(Merge {
            id: merge,
            params: MergeParamsDelegate {
                feather,
                ..Default::default()
            },
            base: PortRef::new(base, "output".to_string()),
            overlay: PortRef::new(overlay, "output".to_string()),
            mask: PortRef::new(mask, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(merge, "output".to_string()),
        }),
    );
    CompileContext::new(&test_composition(graph))
        .compile()
        .unwrap()
}

fn feather_passes(compiled: &lumen_engine::gpu::CompiledComposition) -> usize {
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
}

#[test]
fn merge_without_a_feather_skips_the_gaussian_passes() {
    let size = (MASK_CANVAS_WIDTH, MASK_CANVAS_HEIGHT);
    let sharp = compile_feathered_merge(size, Deferred::value(0.0));
    let negative = compile_feathered_merge(size, Deferred::value(-2.0));
    let expression = compile_feathered_merge(
        size,
        Deferred::Expr(lumen_engine::expr::Expression::parse("0").unwrap()),
    );

    assert_eq!(feather_passes(&sharp), 0);
    assert_eq!(feather_passes(&negative), 0);
    assert!(feather_passes(&expression) > 0);
    let extra_textures = expression.plan.textures().len() - sharp.plan.textures().len();
    assert!(
        extra_textures > 0 && negative.plan.textures().len() == sharp.plan.textures().len(),
        "the expression feather allocates the blur's textures"
    );

    // A feather that evaluates to zero reads the mask exactly like a literal zero, including
    // a translucent straight-alpha mask.
    let rectangle = |graph: &mut Graph| {
        let (shape, mask) = (NodeId::new(5), NodeId::new(6));
        graph
            .nodes
            .insert(shape, edged_rectangle(shape, 8.5, 8, [200, 90, 40, 150]));
        graph.nodes.insert(
            mask,
            NodeKind::AlphaPremultiply(AlphaPremultiply {
                id: mask,
                params: AlphaPremultiplyParamsDelegate {
                    mode: Deferred::value("unpremultiply".to_string()),
                },
                source: PortRef::new(shape, "output".to_string()),
            }),
        );
        mask
    };
    for mask_mode in [MaskMode::Alpha, MaskMode::Luma] {
        let render = |feather: Deferred<f64>| {
            render_matte(
                rectangle,
                MergeParamsDelegate {
                    mask_mode: mask_mode.into(),
                    feather,
                    ..Default::default()
                },
            )
        };
        let skipped = render(Deferred::value(0.0));
        let blurred = render(Deferred::Expr(
            lumen_engine::expr::Expression::parse("0").unwrap(),
        ));
        assert_eq!(skipped, blurred, "{mask_mode:?}");
    }
}

#[test]
fn merge_feather_blurs_the_union_of_the_mask_and_the_base() {
    let feather = || Deferred::value(2.0);
    let output_size = |compiled: &lumen_engine::gpu::CompiledComposition| {
        compiled
            .plan
            .textures()
            .iter()
            .find(|texture| texture.label.as_deref() == Some("merge-feather:3:output"))
            .map(|texture| texture.desc.domain.storage_size)
            .unwrap()
    };
    let base = lumen_gpu::Size::new(MASK_CANVAS_WIDTH as u32, MASK_CANVAS_HEIGHT as u32);

    let larger =
        compile_feathered_merge((MASK_CANVAS_WIDTH + 40, MASK_CANVAS_HEIGHT + 30), feather());
    assert_eq!(
        output_size(&larger),
        lumen_gpu::Size::new(base.width + 40, base.height + 30)
    );
    let wider = compile_feathered_merge((MASK_CANVAS_WIDTH + 8, MASK_CANVAS_HEIGHT / 2), feather());
    assert_eq!(
        output_size(&wider),
        lumen_gpu::Size::new(base.width + 8, base.height)
    );
    let smaller = compile_feathered_merge((12, 12), feather());
    assert_eq!(output_size(&smaller), base);
}

#[test]
fn merge_feather_animates_smoothly() {
    let rectangle = |graph: &mut Graph| {
        let mask = NodeId::new(5);
        graph
            .nodes
            .insert(mask, edged_rectangle(mask, 8.5, 8, [255, 255, 255, 255]));
        mask
    };
    let render = |feather: f64| {
        render_matte(
            rectangle,
            MergeParamsDelegate {
                feather: Deferred::value(feather),
                ..Default::default()
            },
        )
    };
    let mut previous = render(0.0);
    if previous.is_empty() {
        return;
    }
    for step in 1..=32 {
        let feather = f64::from(step) * 0.25;
        let current = render(feather);
        let largest_change = previous
            .iter()
            .zip(&current)
            .map(|(before, after)| before.abs_diff(*after))
            .max()
            .unwrap_or(0);
        assert!(
            largest_change <= 16,
            "feather {} -> {feather} changed a pixel by {largest_change}: {previous:?} -> {current:?}",
            feather - 0.25
        );
        previous = current;
    }
}

#[test]
fn merge_large_feather_has_no_ripples_on_a_thin_line() {
    const WIDTH: usize = 256;
    let line = |graph: &mut Graph| {
        let mask = NodeId::new(5);
        graph
            .nodes
            .insert(mask, edged_rectangle(mask, 128.0, 1, [255, 255, 255, 255]));
        mask
    };
    let blurred = render_matte_on(
        WIDTH,
        line,
        MergeParamsDelegate {
            feather: Deferred::value(32.0),
            ..Default::default()
        },
    );
    if blurred.is_empty() {
        return;
    }
    let peak = blurred[128];
    assert!(
        (3..=9).contains(&peak) && blurred[128 - 24] > 0 && blurred[128 + 24] > 0,
        "the line spreads into a wide, low bump: {blurred:?}"
    );
    // Each side only rises towards the line and then falls away, within a level of rounding.
    let (rising, falling) = (&blurred[..=128], &blurred[128..]);
    assert!(
        rising.windows(2).all(|pair| pair[1] + 1 >= pair[0]),
        "ripples left of the line: {blurred:?}"
    );
    assert!(
        falling.windows(2).all(|pair| pair[0] + 1 >= pair[1]),
        "ripples right of the line: {blurred:?}"
    );
}

fn render_boolean_alpha(a: (f64, i64), b: (f64, i64), params: BooleanParamsDelegate) -> Vec<u8> {
    let (a_id, b_id, boolean, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
    );
    let mut graph = Graph::new();
    graph
        .nodes
        .insert(a_id, edged_rectangle(a_id, a.0, a.1, [255, 255, 255, 255]));
    graph
        .nodes
        .insert(b_id, edged_rectangle(b_id, b.0, b.1, [255, 255, 255, 255]));
    graph.nodes.insert(
        boolean,
        NodeKind::Boolean(Boolean {
            id: boolean,
            params,
            a: PortRef::new(a_id, "output".to_string()),
            b: PortRef::new(b_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(boolean, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = MASK_CANVAS_WIDTH as u32;
    composition.render_settings.height = MASK_CANVAS_HEIGHT as u32;
    let Some(mut renderer) = gpu_composition_renderer() else {
        return Vec::new();
    };
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    (0..MASK_CANVAS_WIDTH)
        .map(|x| pixels[(MASK_ROW * MASK_CANVAS_WIDTH + x) * 4 + 3])
        .collect()
}

#[test]
fn boolean_preserves_antialiased_edges_by_default() {
    // A covers columns 1 (half), 2..=3 and 4 (half); B covers 3 (half), 4..=7 and 8 (half).
    let (a, b) = ((1.5, 3), (3.5, 5));
    let union = render_boolean_alpha(a, b, BooleanParamsDelegate::default());
    let intersect = render_boolean_alpha(
        a,
        b,
        BooleanParamsDelegate {
            operation: Deferred::value(BooleanOperation::Intersect as i64),
            ..Default::default()
        },
    );
    if union.is_empty() {
        return;
    }
    assert_matte(
        &[union.clone(), intersect.clone()].concat(),
        &[
            (0, 0),
            (1, 128),
            (2, 255),
            (3, 255),
            (4, 255),
            (8, 128),
            (9, 0),
            (MASK_CANVAS_WIDTH + 3, 128),
            (MASK_CANVAS_WIDTH + 4, 128),
            (MASK_CANVAS_WIDTH + 2, 0),
            (MASK_CANVAS_WIDTH + 5, 0),
        ],
    );

    // An explicit threshold still binarizes the pixels above it.
    let thresholded = render_boolean_alpha(
        a,
        b,
        BooleanParamsDelegate {
            threshold: Deferred::value(0.25),
            ..Default::default()
        },
    );
    assert_matte(&thresholded, &[(1, 255), (4, 255), (8, 255), (9, 0)]);
}

#[test]
fn boolean_subtracting_a_mask_from_itself_leaves_nothing() {
    let subtract = BooleanParamsDelegate {
        operation: Deferred::value(BooleanOperation::Subtract as i64),
        ..Default::default()
    };
    // Columns 1 and 4 are half covered.
    let result = render_boolean_alpha((1.5, 3), (1.5, 3), subtract.clone());
    if result.is_empty() {
        return;
    }
    assert_eq!(result, vec![0; MASK_CANVAS_WIDTH]);

    // Subtracting a partially overlapping mask keeps only what B does not cover.
    let result = render_boolean_alpha((1.5, 3), (3.5, 5), subtract);
    assert_matte(&result, &[(1, 128), (2, 255), (3, 128), (4, 0)]);
}

#[test]
fn zero_size_shape_draws_nothing() {
    for (width, height) in [(0, 4), (8, 0), (0, 0)] {
        let shape = NodeId::new(1);
        let output = NodeId::new(2);
        let mut graph = Graph::new();
        graph.nodes.insert(
            shape,
            NodeKind::Shape(Shape {
                id: shape,
                params: ShapeParamsDelegate {
                    width: Deferred::value(width),
                    height: Deferred::value(height),
                    stroke_enabled: Deferred::value(true),
                    stroke_paint: Paint::solid([0, 255, 0, 255]).into(),
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
        let Some(mut renderer) = gpu_composition_renderer() else {
            return;
        };
        let pixels = render_rgba8(&mut renderer, &test_composition(graph), 0, &EmptyMediaStore);
        assert!(
            pixels.iter().all(|&channel| channel == 0),
            "{width}x{height} shape drew something: {pixels:?}"
        );
    }
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

fn render_drawable_graph(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    nodes: Vec<NodeKind>,
    size: u32,
) -> Vec<u8> {
    let mut graph = Graph::new();
    let mut last = NodeId::new(0);
    for node in nodes {
        last = node.id();
        graph.nodes.insert(last, node);
    }
    let output = NodeId::new(100);
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(last, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = size;
    composition.render_settings.height = size;
    renderer
        .compile_with_media(
            &composition,
            &EmptyMediaStore,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .unwrap();
    render_rgba8(renderer, &composition, 0, &EmptyMediaStore)
}

fn max_channel_difference(a: &[u8], b: &[u8]) -> u8 {
    a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap()
}

fn partially_covered(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] > 0 && pixel[3] < 255)
        .count()
}

#[test]
fn shape_scaled_through_its_own_transform_stays_pixel_sharp() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shape = |params: ShapeParamsDelegate| {
        vec![NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params,
        })]
    };
    let cases = [
        (
            "stroked ellipse, uniform 8x",
            2,
            ShapeParamsDelegate {
                geometry_kind: ShapeGeometryKind::Ellipse.into(),
                width: Deferred::value(10),
                height: Deferred::value(9),
                position: Deferred::value((20.25, 30.5)),
                scale: Deferred::value((8.0, 8.0)),
                stroke_enabled: Deferred::value(true),
                stroke_width: Deferred::value(0.75),
                fill_paint: Paint::solid([255, 0, 0, 255]).into(),
                stroke_paint: Paint::solid([0, 0, 255, 255]).into(),
                ..Default::default()
            },
            ShapeParamsDelegate {
                geometry_kind: ShapeGeometryKind::Ellipse.into(),
                width: Deferred::value(80),
                height: Deferred::value(72),
                position: Deferred::value((20.25, 30.5)),
                stroke_enabled: Deferred::value(true),
                stroke_width: Deferred::value(6.0),
                fill_paint: Paint::solid([255, 0, 0, 255]).into(),
                stroke_paint: Paint::solid([0, 0, 255, 255]).into(),
                ..Default::default()
            },
        ),
        // Corner distances under non-uniform scale are a first-order estimate.
        (
            "rectangle, non-uniform 8x2 about an anchor",
            32,
            ShapeParamsDelegate {
                width: Deferred::value(10),
                height: Deferred::value(30),
                position: Deferred::value((60.3, 40.6)),
                anchor: Deferred::value((5.0, 15.0)),
                scale: Deferred::value((8.0, 2.0)),
                ..Default::default()
            },
            ShapeParamsDelegate {
                width: Deferred::value(80),
                height: Deferred::value(60),
                position: Deferred::value((25.3, 25.6)),
                ..Default::default()
            },
        ),
    ];
    for (name, tolerance, scaled, direct) in cases {
        let scaled = render_drawable_graph(&mut renderer, shape(scaled), 128);
        let direct = render_drawable_graph(&mut renderer, shape(direct), 128);
        assert!(
            partially_covered(&direct) > 0,
            "{name}: no antialiased edge"
        );
        assert!(
            partially_covered(&scaled).abs_diff(partially_covered(&direct)) <= 2,
            "{name}: edge width"
        );
        assert!(
            max_channel_difference(&scaled, &direct) <= tolerance,
            "{name}: scaled shape differs from the shape drawn at its size"
        );
    }
}

#[test]
fn non_uniform_scale_antialiases_the_nearest_canvas_edge() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shape = |width: i64, height: i64, scale: (f64, f64)| {
        vec![NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params: ShapeParamsDelegate {
                width: Deferred::value(width),
                height: Deferred::value(height),
                position: Deferred::value((8.0, 8.2)),
                scale: Deferred::value(scale),
                fill_paint: Paint::solid([255, 255, 255, 255]).into(),
                ..Default::default()
            },
        })]
    };
    let path = |data: &str, scale: (f64, f64)| {
        vec![NodeKind::Path(Path {
            id: NodeId::new(1),
            params: PathParamsDelegate {
                data: Deferred::value(data.to_string()),
                position: Deferred::value((8.0, 8.2)),
                scale: Deferred::value(scale),
                fill_enabled: Deferred::value(true),
                fill_paint: Paint::solid([255, 255, 255, 255]).into(),
                ..Default::default()
            },
        })]
    };
    let reference = render_drawable_graph(&mut renderer, shape(100, 10, (1.0, 1.0)), 128);
    let scaled_shape = render_drawable_graph(&mut renderer, shape(1, 100, (100.0, 0.1)), 128);
    let scaled_path =
        render_drawable_graph(&mut renderer, path("M0 0 H1 V100 H0 Z", (100.0, 0.1)), 128);
    let alpha = |pixels: &[u8], x: usize, y: usize| pixels[(y * 128 + x) * 4 + 3];
    assert_eq!(
        alpha(&reference, 58, 8),
        204,
        "the reference edge is partial"
    );
    for (name, pixels) in [("shape", &scaled_shape), ("path", &scaled_path)] {
        assert!(
            max_channel_difference(pixels, &reference) <= 2,
            "{name}: differs from the unscaled rectangle by {}",
            max_channel_difference(pixels, &reference),
        );
    }
}

/// Maps a canvas point to untransformed pixels with the drawable transform's conventions.
fn drawable_to_local(
    point: (f64, f64),
    pivot: (f64, f64),
    scale: (f64, f64),
    rotation: f64,
) -> (f64, f64) {
    let (sin, cos) = rotation.to_radians().sin_cos();
    let (dx, dy) = (point.0 - pivot.0, point.1 - pivot.1);
    (
        pivot.0 + (cos * dx + sin * dy) / scale.0,
        pivot.1 + (cos * dy - sin * dx) / scale.1,
    )
}

/// Alpha of an outline in untransformed pixels, drawn about `pivot` and supersampled
/// per canvas pixel. The samples are a sphere of radius half a pixel projected onto the
/// canvas, so a straight edge at any angle covers them linearly across one pixel, as
/// analytic antialiasing does, and any difference comes from the distance estimate.
fn supersampled_alpha(
    size: usize,
    pivot: (f64, f64),
    scale: (f64, f64),
    rotation: f64,
    inside: &dyn Fn(f64, f64) -> bool,
) -> Vec<u8> {
    const RINGS: usize = 32;
    const ANGLES: usize = 16;
    let golden_angle = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let offsets: Vec<(f64, f64)> = (0..RINGS * ANGLES)
        .map(|sample| {
            let (ring, angle) = (sample / ANGLES, sample % ANGLES);
            let height = (ring as f64 + 0.5) / RINGS as f64;
            let radius = 0.5 * (1.0 - height * height).sqrt();
            let angle = std::f64::consts::TAU * (angle as f64 + 0.5) / ANGLES as f64
                + ring as f64 * golden_angle;
            (0.5 + radius * angle.cos(), 0.5 + radius * angle.sin())
        })
        .collect();
    let inside_canvas = |point: (f64, f64)| {
        let (x, y) = drawable_to_local(point, pivot, scale, rotation);
        inside(x, y)
    };
    (0..size * size)
        .map(|index| {
            let origin = ((index % size) as f64, (index / size) as f64);
            let centre = inside_canvas((origin.0 + 0.5, origin.1 + 0.5));
            // Pixels whose surroundings out to a pixel agree with their centre are uniform.
            let uniform = (0..16).all(|step| {
                let angle = std::f64::consts::TAU * step as f64 / 16.0;
                inside_canvas((origin.0 + 0.5 + angle.cos(), origin.1 + 0.5 + angle.sin()))
                    == centre
            });
            if uniform {
                return if centre { 255 } else { 0 };
            }
            let covered = offsets
                .iter()
                .filter(|(dx, dy)| inside_canvas((origin.0 + dx, origin.1 + dy)))
                .count();
            (covered as f64 * 255.0 / offsets.len() as f64).round() as u8
        })
        .collect()
}

#[test]
fn non_uniform_scale_matches_a_supersampled_outline() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    const SIZE: usize = 256;
    let square = ShapeParamsDelegate {
        width: Deferred::value(40),
        height: Deferred::value(40),
        position: Deferred::value((50.3, 40.2)),
        fill_paint: Paint::solid([255, 255, 255, 255]).into(),
        stroke_paint: Paint::solid([255, 255, 255, 255]).into(),
        ..Default::default()
    };
    let shape = |params: ShapeParamsDelegate, scale: (f64, f64), rotation: f64| {
        NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params: ShapeParamsDelegate {
                scale: Deferred::value(scale),
                rotation: Deferred::value(rotation),
                ..params
            },
        })
    };
    let path = |scale: (f64, f64), rotation: f64| {
        NodeKind::Path(Path {
            id: NodeId::new(1),
            params: PathParamsDelegate {
                data: Deferred::value("M5 20 L35 20".to_string()),
                position: Deferred::value((55.0, 60.0)),
                anchor: Deferred::value((20.0, 20.0)),
                scale: Deferred::value(scale),
                rotation: Deferred::value(rotation),
                fill_enabled: Deferred::value(false),
                stroke_enabled: Deferred::value(true),
                stroke_width: Deferred::value(4.0),
                stroke_paint: Paint::solid([255, 255, 255, 255]).into(),
                ..Default::default()
            },
        })
    };
    // Untransformed outlines; the shapes are centred on (70.3, 60.2).
    let centred = |x: f64, y: f64| ((x - 70.3).abs(), (y - 60.2).abs());
    let capsule = |x: f64, y: f64| (x - x.clamp(60.0, 90.0)).hypot(y - 80.0) <= 2.0;
    let stroked_rect = |x: f64, y: f64| {
        let (x, y) = centred(x, y);
        (x - 20.0).max(0.0).hypot((y - 20.0).max(0.0)) <= 2.0 && !(x < 18.0 && y < 18.0)
    };
    let rounded_rect = |x: f64, y: f64| {
        let (x, y) = centred(x, y);
        (x - 5.0).max(0.0).hypot((y - 5.0).max(0.0)) <= 15.0
    };
    let ellipse = |x: f64, y: f64| {
        let (x, y) = centred(x, y);
        (x / 20.0).powi(2) + (y / 10.0).powi(2) <= 1.0
    };
    // Sharp corners have no distance gradient, so analytic antialiasing cannot match
    // area coverage within a pixel of them. Under scale (4, 1) the stroke's ends and outer
    // corners curve with a radius of half a canvas pixel, where even the exact distance
    // differs from coverage by about 28, so those cases get a looser tolerance.
    let inner_corners = [(-18.0, -18.0), (18.0, -18.0), (-18.0, 18.0), (18.0, 18.0)]
        .map(|(x, y)| (70.3 + x, 60.2 + y));

    for (scale, rotation) in [((1.0, 1.0), 0.0), ((4.0, 1.0), 0.0), ((4.0, 1.0), 25.0)] {
        let stroke_tolerance = if scale == (1.0, 1.0) { 10 } else { 32 };
        #[allow(clippy::type_complexity)]
        let cases: [(
            &str,
            NodeKind,
            (f64, f64),
            &dyn Fn(f64, f64) -> bool,
            &[(f64, f64)],
            u8,
        ); 4] = [
            (
                "path caps",
                path(scale, rotation),
                (75.0, 80.0),
                &capsule,
                &[],
                stroke_tolerance,
            ),
            (
                "stroked rectangle",
                shape(
                    ShapeParamsDelegate {
                        fill_enabled: Deferred::value(false),
                        stroke_enabled: Deferred::value(true),
                        stroke_width: Deferred::value(4.0),
                        ..square.clone()
                    },
                    scale,
                    rotation,
                ),
                (50.3, 40.2),
                &stroked_rect,
                &inner_corners,
                stroke_tolerance,
            ),
            (
                "rounded rectangle",
                shape(
                    ShapeParamsDelegate {
                        border_radius: Deferred::value(15.0),
                        ..square.clone()
                    },
                    scale,
                    rotation,
                ),
                (50.3, 40.2),
                &rounded_rect,
                &[],
                8,
            ),
            (
                "ellipse",
                shape(
                    ShapeParamsDelegate {
                        geometry_kind: ShapeGeometryKind::Ellipse.into(),
                        height: Deferred::value(20),
                        position: Deferred::value((50.3, 50.2)),
                        ..square.clone()
                    },
                    scale,
                    rotation,
                ),
                (50.3, 50.2),
                &ellipse,
                &[],
                12,
            ),
        ];
        for (name, node, pivot, inside, sharp_corners, tolerance) in cases {
            let pixels = render_drawable_graph(&mut renderer, vec![node], SIZE as u32);
            let reference = supersampled_alpha(SIZE, pivot, scale, rotation, inside);
            let near_sharp_corner = |index: usize| {
                let centre = ((index % SIZE) as f64 + 0.5, (index / SIZE) as f64 + 0.5);
                sharp_corners.iter().any(|&corner| {
                    let (x, y) = drawable_to_local(centre, pivot, scale, rotation);
                    let local = (x - corner.0, y - corner.1);
                    // Back to canvas pixels about the corner.
                    let (sin, cos) = rotation.to_radians().sin_cos();
                    let (dx, dy) = (local.0 * scale.0, local.1 * scale.1);
                    (cos * dx - sin * dy).hypot(sin * dx + cos * dy) < 1.5
                })
            };
            let (error, at) = pixels
                .iter()
                .skip(3)
                .step_by(4)
                .zip(&reference)
                .enumerate()
                .filter(|(index, _)| !near_sharp_corner(*index))
                .map(|(index, (gpu, reference))| (gpu.abs_diff(*reference), index))
                .max()
                .unwrap();
            let case = format!("{name} at scale {scale:?}, {rotation} degrees");
            eprintln!(
                "{case}: max alpha error {error} at ({}, {})",
                at % SIZE,
                at / SIZE
            );
            assert!(reference.contains(&255), "{case}: off canvas");
            assert!(
                error <= tolerance,
                "{case}: alpha differs by {error} at ({}, {})",
                at % SIZE,
                at / SIZE
            );
            if name == "stroked rectangle" && scale == (4.0, 1.0) && rotation == 0.0 {
                assert_eq!(
                    pixels[(43 * SIZE + 54) * 4 + 3],
                    255,
                    "{case}: inside corner"
                );
            }
        }
    }
}

/// Signed distance from `(x, y)` to the ellipse with radii `(a, b)` about the origin. The
/// nearest point is `(a² x / (t + a²), b² y / (t + b²))` for the root `t` of the function
/// below, which falls from infinity to -1 as `t` rises from `-min(a, b)²`.
fn ellipse_distance(x: f64, y: f64, a: f64, b: f64) -> f64 {
    let (x, y) = (x.abs().max(1e-9), y.abs().max(1e-9));
    let nearest = |t: f64| (a * a * x / (t + a * a), b * b * y / (t + b * b));
    let (mut low, mut high) = (-a.min(b).powi(2), a.max(b) * x.hypot(y));
    for _ in 0..80 {
        let t = 0.5 * (low + high);
        let (px, py) = nearest(t);
        if (px / a).powi(2) + (py / b).powi(2) > 1.0 {
            low = t;
        } else {
            high = t;
        }
    }
    let (px, py) = nearest(0.5 * (low + high));
    (x - px).hypot(y - py) * if low < 0.0 { -1.0 } else { 1.0 }
}

#[test]
fn wide_ellipse_strokes_compressed_caps_and_slivers_stay_exact() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    const SIZE: usize = 256;
    let ellipse = |params: ShapeParamsDelegate| {
        NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params: ShapeParamsDelegate {
                geometry_kind: ShapeGeometryKind::Ellipse.into(),
                width: Deferred::value(40),
                height: Deferred::value(20),
                fill_paint: Paint::solid([255, 255, 255, 255]).into(),
                stroke_paint: Paint::solid([255, 255, 255, 255]).into(),
                ..params
            },
        })
    };
    // A stroke wider than the ellipse is tall covers its whole inside.
    let stroke = ellipse(ShapeParamsDelegate {
        position: Deferred::value((50.5, 40.5)),
        fill_enabled: Deferred::value(false),
        stroke_enabled: Deferred::value(true),
        stroke_width: Deferred::value(24.0),
        ..Default::default()
    });
    let stroke_distance =
        |x: f64, y: f64| ellipse_distance(x - 70.5, y - 50.5, 20.0, 10.0).abs() - 12.0;
    // Scaled by (4, 0.1) about its start, the stroke is a band 0.4 canvas pixels tall
    // from x = 70.5 to 190.5, and each cap an ellipse with radii (8, 0.2).
    let compressed = NodeKind::Path(Path {
        id: NodeId::new(1),
        params: PathParamsDelegate {
            data: Deferred::value("M5 20 L35 20".to_string()),
            position: Deferred::value((50.5, 40.5)),
            anchor: Deferred::value((0.0, 20.0)),
            scale: Deferred::value((4.0, 0.1)),
            fill_enabled: Deferred::value(false),
            stroke_enabled: Deferred::value(true),
            stroke_width: Deferred::value(4.0),
            stroke_paint: Paint::solid([255, 255, 255, 255]).into(),
            ..Default::default()
        },
    });
    let capsule_distance = |x: f64, y: f64| {
        let (x, y) = (x - 130.5, y - 60.5);
        let (bx, by) = (x.abs() - 60.0, y.abs() - 0.2);
        let band = bx.max(0.0).hypot(by.max(0.0)) + bx.max(by).min(0.0);
        let caps =
            ellipse_distance(x + 60.0, y, 8.0, 0.2).min(ellipse_distance(x - 60.0, y, 8.0, 0.2));
        band.min(caps)
    };
    // Compared with the exact canvas distance ramp at each pixel centre, so any
    // difference comes from the shader's distance.
    for (name, node, distance) in [
        (
            "wide ellipse stroke",
            stroke,
            &stroke_distance as &dyn Fn(f64, f64) -> f64,
        ),
        ("compressed caps", compressed, &capsule_distance),
    ] {
        let pixels = render_drawable_graph(&mut renderer, vec![node], SIZE as u32);
        let (error, at) = pixels
            .iter()
            .skip(3)
            .step_by(4)
            .enumerate()
            .map(|(index, gpu)| {
                let centre = ((index % SIZE) as f64 + 0.5, (index / SIZE) as f64 + 0.5);
                let exact = (0.5 - distance(centre.0, centre.1)).clamp(0.0, 1.0) * 255.0;
                (gpu.abs_diff(exact.round() as u8), index)
            })
            .max()
            .unwrap();
        eprintln!(
            "{name}: max alpha error {error} at ({}, {})",
            at % SIZE,
            at / SIZE
        );
        assert!(
            error <= 2,
            "{name}: alpha differs by {error} at ({}, {})",
            at % SIZE,
            at / SIZE
        );
    }

    // A sliver a millionth of a pixel wide covers at most half of the pixels it crosses
    // and nothing beyond its ends.
    let sliver = ellipse(ShapeParamsDelegate {
        position: Deferred::value((128.5, 128.5)),
        anchor: Deferred::value((20.0, 10.0)),
        scale: Deferred::value((0.000001, 1.0)),
        ..Default::default()
    });
    let pixels = render_drawable_graph(&mut renderer, vec![sliver], SIZE as u32);
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (index % SIZE, index / SIZE);
        let limit = if x == 148 && (128..=148).contains(&y) {
            128
        } else {
            0
        };
        assert!(
            pixel[3] <= limit,
            "sliver: alpha {} at ({x}, {y})",
            pixel[3]
        );
    }
}

#[test]
fn eccentric_ellipse_tips_stay_exact() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    const SIZE: usize = 256;
    let mut failures = Vec::new();
    // Each ellipse has a tip, on its outline, at the centre of pixel (100, 100), and its
    // body runs along the row or column through it, so the neighbouring pixels are inside
    // the ellipse close to its major axis.
    for (name, width, height, position, centre) in [
        ("2000x2", 2000, 2, (-1899.5, 99.5), (-899.5, 100.5)),
        ("20000x100", 20000, 100, (-19899.5, 50.5), (-9899.5, 100.5)),
        ("2x2000", 2, 2000, (99.5, -1899.5), (100.5, -899.5)),
    ] {
        let node = NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params: ShapeParamsDelegate {
                geometry_kind: ShapeGeometryKind::Ellipse.into(),
                width: Deferred::value(width),
                height: Deferred::value(height),
                position: Deferred::value(position),
                fill_paint: Paint::solid([255, 255, 255, 255]).into(),
                ..Default::default()
            },
        });
        let pixels = render_drawable_graph(&mut renderer, vec![node], SIZE as u32);
        let (error, at) = pixels
            .iter()
            .skip(3)
            .step_by(4)
            .enumerate()
            .map(|(index, gpu)| {
                let (x, y) = ((index % SIZE) as f64 + 0.5, (index / SIZE) as f64 + 0.5);
                let distance = ellipse_distance(
                    x - centre.0,
                    y - centre.1,
                    width as f64 / 2.0,
                    height as f64 / 2.0,
                );
                let exact = (0.5 - distance).clamp(0.0, 1.0) * 255.0;
                (gpu.abs_diff(exact.round() as u8), index)
            })
            .max()
            .unwrap();
        eprintln!(
            "{name}: max alpha error {error} at ({}, {})",
            at % SIZE,
            at / SIZE
        );
        if error > 2 {
            failures.push(format!("{name}: alpha differs by {error}"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn eccentric_stroked_tip_keeps_its_corner_pixels() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // The right tip sits at (99.5, 101.5); pixel (100, 100) is 1.41 px from it, inside the
    // 2.5 px half stroke.
    let node = NodeKind::Shape(Shape {
        id: NodeId::new(1),
        params: ShapeParamsDelegate {
            geometry_kind: ShapeGeometryKind::Ellipse.into(),
            width: Deferred::value(68797),
            height: Deferred::value(1),
            position: Deferred::value((-68697.5, 101.0)),
            fill_enabled: Deferred::value(false),
            stroke_enabled: Deferred::value(true),
            stroke_width: Deferred::value(5.0),
            ..Default::default()
        },
    });
    let pixels = render_drawable_graph(&mut renderer, vec![node], 256);
    assert_eq!(pixels[(100 * 256 + 100) * 4 + 3], 255);

    // Squashed vertically, the tip's falloff runs diagonally, so the antialiasing ramp
    // must follow the displacement rather than the tip's horizontal surface normal.
    let node = NodeKind::Shape(Shape {
        id: NodeId::new(1),
        params: ShapeParamsDelegate {
            geometry_kind: ShapeGeometryKind::Ellipse.into(),
            width: Deferred::value(68797),
            height: Deferred::value(1),
            position: Deferred::value((-68697.5, 100.1)),
            anchor: Deferred::value((68797.0, 0.5)),
            scale: Deferred::value((1.0, 0.1)),
            fill_enabled: Deferred::value(false),
            stroke_enabled: Deferred::value(true),
            stroke_width: Deferred::value(2.5),
            ..Default::default()
        },
    });
    let pixels = render_drawable_graph(&mut renderer, vec![node], 256);
    let alpha = pixels[(100 * 256 + 100) * 4 + 3];
    assert!(
        alpha.abs_diff(121) <= 6,
        "squashed tip alpha {alpha}, expected about 121"
    );

    // A thousandth of a pixel of motion must not flip the edge between two normals.
    let mut squashed_at = |x: f64| {
        let node = NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params: ShapeParamsDelegate {
                geometry_kind: ShapeGeometryKind::Ellipse.into(),
                width: Deferred::value(68797),
                height: Deferred::value(1),
                position: Deferred::value((x, 99.9255)),
                anchor: Deferred::value((34398.5, 0.5)),
                scale: Deferred::value((1.0, 0.1)),
                fill_enabled: Deferred::value(false),
                stroke_enabled: Deferred::value(true),
                stroke_width: Deferred::value(2.5),
                ..Default::default()
            },
        });
        render_drawable_graph(&mut renderer, vec![node], 256)[(100 * 256 + 100) * 4 + 3]
    };
    let (before, after) = (squashed_at(-68697.378), squashed_at(-68697.379));
    assert!(
        before.abs_diff(after) <= 3,
        "alpha jumped {before} -> {after}"
    );
}

#[test]
fn near_circle_stroke_stays_empty_near_the_centre() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    const SIZE: usize = 256;
    // Positions a hair apart put pixel (102, 100) on either side of the evolute cusp, where
    // the iteration used to collapse onto the centre and draw a stray opaque pixel.
    for x in [-0.4889678955078125, -0.48895263671875, -0.4889831542968750] {
        let node = NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params: ShapeParamsDelegate {
                geometry_kind: ShapeGeometryKind::Ellipse.into(),
                width: Deferred::value(202),
                height: Deferred::value(200),
                position: Deferred::value((x, 0.5)),
                fill_enabled: Deferred::value(false),
                stroke_enabled: Deferred::value(true),
                stroke_width: Deferred::value(5.0),
                ..Default::default()
            },
        });
        let pixels = render_drawable_graph(&mut renderer, vec![node], SIZE as u32);
        let stray = pixels
            .iter()
            .skip(3)
            .step_by(4)
            .enumerate()
            .filter(|&(index, &alpha)| {
                let (px, py) = ((index % SIZE) as f64 + 0.5, (index / SIZE) as f64 + 0.5);
                let distance = ellipse_distance(px - (x + 101.0), py - 100.5, 101.0, 100.0);
                distance < -10.0 && alpha != 0
            })
            .map(|(index, _)| (index % SIZE, index / SIZE))
            .collect::<Vec<_>>();
        assert!(
            stray.is_empty(),
            "x = {x}: stray stroke pixels at {stray:?}"
        );
    }
}

#[test]
fn shape_rotation_and_anchor_match_the_transform_node() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shape = |transformed: bool| {
        NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params: ShapeParamsDelegate {
                width: Deferred::value(48),
                height: Deferred::value(24),
                border_radius: Deferred::value(6.0),
                position: Deferred::value((30.0, 50.0)),
                stroke_enabled: Deferred::value(true),
                stroke_width: Deferred::value(3.0),
                fill_paint: Paint::solid([255, 128, 0, 255]).into(),
                stroke_paint: Paint::solid([0, 64, 255, 255]).into(),
                scale: Deferred::value(if transformed { (1.25, 0.8) } else { (1.0, 1.0) }),
                rotation: Deferred::value(if transformed { 30.0 } else { 0.0 }),
                anchor: Deferred::value((12.0, 6.0)),
                ..Default::default()
            },
        })
    };
    let own = render_drawable_graph(&mut renderer, vec![shape(true)], 128);
    let via_node = render_drawable_graph(
        &mut renderer,
        vec![
            shape(false),
            NodeKind::Transform(Transform {
                id: NodeId::new(2),
                params: TransformParamsDelegate {
                    scale_x: Deferred::value(1.25),
                    scale_y: Deferred::value(0.8),
                    rotate: Deferred::value(30.0),
                    pivot_x: Deferred::value(42.0),
                    pivot_y: Deferred::value(56.0),
                    custom_pivot: Deferred::value(true),
                    ..Default::default()
                },
                source: PortRef::new(NodeId::new(1), "output".to_string()),
            }),
        ],
        128,
    );

    let alpha = |pixels: &[u8]| -> u64 {
        pixels
            .iter()
            .skip(3)
            .step_by(4)
            .map(|&a| u64::from(a))
            .sum()
    };
    let mean_difference = own
        .iter()
        .zip(&via_node)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum::<u64>() as f64
        / own.len() as f64;
    eprintln!(
        "rotation: mean difference {mean_difference:.3}, max {}",
        max_channel_difference(&own, &via_node)
    );
    assert!(alpha(&own).abs_diff(alpha(&via_node)) * 100 < alpha(&via_node));
    assert!(mean_difference < 1.0, "mean difference {mean_difference}");
    // Only the resampled edges of the raster may differ noticeably.
    assert!(max_channel_difference(&own, &via_node) < 128);
}

#[test]
fn drawable_opacity_scales_premultiplied_output() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let gradient = Paint::Gradient(GradientPaint {
        kind: PaintKind::LinearGradient,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: [255, 0, 0, 255],
            },
            GradientStop {
                offset: 1.0,
                color: [0, 0, 255, 128],
            },
        ],
        ..Default::default()
    });
    let nodes = |opacity: f64| {
        [
            NodeKind::Shape(Shape {
                id: NodeId::new(1),
                params: ShapeParamsDelegate {
                    geometry_kind: ShapeGeometryKind::Ellipse.into(),
                    width: Deferred::value(40),
                    height: Deferred::value(30),
                    position: Deferred::value((4.5, 6.0)),
                    fill_paint: gradient.clone().into(),
                    opacity: Deferred::value(opacity),
                    ..Default::default()
                },
            }),
            NodeKind::Path(Path {
                id: NodeId::new(1),
                params: PathParamsDelegate {
                    data: Deferred::value("M 5 5 Q 40 0 45 40 L 5 45 Z".to_string()),
                    fill_paint: gradient.clone().into(),
                    stroke_enabled: Deferred::value(true),
                    stroke_width: Deferred::value(2.0),
                    opacity: Deferred::value(opacity),
                    ..Default::default()
                },
            }),
        ]
    };
    for (opaque, translucent) in nodes(1.0).into_iter().zip(nodes(0.5)) {
        let opaque = render_drawable_graph(&mut renderer, vec![opaque], 50);
        let translucent = render_drawable_graph(&mut renderer, vec![translucent], 50);
        assert!(opaque.iter().skip(3).step_by(4).any(|&a| a > 200));
        for (full, half) in opaque.iter().zip(&translucent) {
            assert!(
                (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= 1.0,
                "{full} at half opacity gave {half}"
            );
        }
    }
}

#[test]
fn text_transform_in_msdf_mode_matches_larger_and_rotated_text() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = |font_size: f64, scale: f64, rotation: f64, anchor: (f64, f64)| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value("Lumen AV".to_string()),
                font_size: Deferred::value(font_size),
                render_mode: TextRenderMode::Msdf.into(),
                position: Deferred::value((20.0, 30.0)),
                scale: Deferred::value((scale, scale)),
                rotation: Deferred::value(rotation),
                anchor: Deferred::value(anchor),
                ..Default::default()
            },
        })]
    };
    let size = 256;
    let coverage =
        |pixels: &[u8]| -> Vec<u8> { pixels.iter().skip(3).step_by(4).copied().collect() };
    let iou = |a: &[u8], b: &[u8]| {
        let (intersection, union) = a.iter().zip(b).fold((0u64, 0u64), |(i, u), (a, b)| {
            (i + u64::from(*a.min(b)), u + u64::from(*a.max(b)))
        });
        intersection as f64 / union as f64
    };

    // Scaled glyphs keep MSDF edges as sharp as glyphs laid out at the larger size.
    let scaled = coverage(&render_drawable_graph(
        &mut renderer,
        text(16.0, 3.0, 0.0, (0.0, 0.0)),
        size,
    ));
    let larger = coverage(&render_drawable_graph(
        &mut renderer,
        text(48.0, 1.0, 0.0, (0.0, 0.0)),
        size,
    ));
    let scaled_edges = scaled.iter().filter(|&&a| a > 0 && a < 255).count();
    let larger_edges = larger.iter().filter(|&&a| a > 0 && a < 255).count();
    eprintln!(
        "msdf scale: IoU {:.3}, edges {scaled_edges} vs {larger_edges}",
        iou(&scaled, &larger)
    );
    assert!(iou(&scaled, &larger) > 0.9);
    assert!(scaled_edges.abs_diff(larger_edges) * 4 < larger_edges);

    // A quarter turn about a whole-pixel anchor moves pixel centres onto pixel centres.
    let upright = coverage(&render_drawable_graph(
        &mut renderer,
        text(24.0, 2.0, 0.0, (40.0, 20.0)),
        size,
    ));
    let turned = coverage(&render_drawable_graph(
        &mut renderer,
        text(24.0, 2.0, 90.0, (40.0, 20.0)),
        size,
    ));
    let (pivot_x, pivot_y) = (60i64, 50i64);
    let mut expected = vec![0u8; turned.len()];
    for y in 0..size as i64 {
        for x in 0..size as i64 {
            let (tx, ty) = (pivot_x - (y - pivot_y) - 1, pivot_y + (x - pivot_x));
            if (0..size as i64).contains(&tx) && (0..size as i64).contains(&ty) {
                expected[(ty * size as i64 + tx) as usize] =
                    upright[(y * size as i64 + x) as usize];
            }
        }
    }
    let turned_iou = iou(&turned, &expected);
    eprintln!("msdf rotation: IoU {turned_iou:.3}");
    assert!(upright.iter().any(|&a| a == 255));
    assert!(turned_iou > 0.97);
}

#[test]
fn path_with_more_than_128_points_renders_fully_and_errors_past_the_maximum() {
    let polyline = |count: usize| {
        let mut data = String::from("M 10 10");
        for index in 1..count - 2 {
            let x = 10.0 + 100.0 * index as f64 / (count - 3) as f64;
            data += &format!(" L {x:.4} {}", if index % 2 == 0 { 10.0 } else { 11.0 });
        }
        data + " L 110 90 L 10 90 Z"
    };
    let path = |data: String| {
        NodeKind::Path(Path {
            id: NodeId::new(1),
            params: PathParamsDelegate {
                data: Deferred::value(data),
                ..Default::default()
            },
        })
    };

    let mut graph = Graph::new();
    graph.nodes.insert(NodeId::new(1), path(polyline(20_000)));
    graph.nodes.insert(
        NodeId::new(2),
        NodeKind::MediaOutput(MediaOutput {
            id: NodeId::new(2),
            source: PortRef::new(NodeId::new(1), "output".to_string()),
        }),
    );
    let composition = test_composition(graph);
    let compiled = CompileContext::new(&composition).compile().unwrap();
    let error = FrameBindContext::new(&composition, 0)
        .bind(&compiled)
        .unwrap_err();
    assert!(
        error.to_string().contains("more than 16384 points"),
        "{error}"
    );

    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let pixels = render_drawable_graph(&mut renderer, vec![path(polyline(400))], 128);
    let points_buffer = renderer
        .compiled()
        .unwrap()
        .plan
        .buffers()
        .iter()
        .find(|buffer| buffer.label.as_deref() == Some("path:1:points"))
        .unwrap()
        .desc
        .size;
    assert_eq!(points_buffer, 400 * 24, "sized to the flattened path");
    let alpha = |x: usize, y: usize| pixels[(y * 128 + x) * 4 + 3];
    for (x, y) in [(15, 50), (60, 50), (105, 50), (105, 85)] {
        assert_eq!(alpha(x, y), 255, "({x}, {y}) is inside the path");
    }
    for (x, y) in [(5, 50), (115, 50), (60, 95)] {
        assert_eq!(alpha(x, y), 0, "({x}, {y}) is outside the path");
    }
}

#[test]
fn banded_path_fill_keeps_holes_and_self_intersections() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // An outer square, then a pentagram: even-odd leaves its points as holes and fills
    // its centre. Edges are subdivided so the path spans many bands.
    let star: Vec<[f64; 2]> = (0..5)
        .map(|index| {
            let angle = (index * 2) as f64 * std::f64::consts::TAU / 5.0;
            [64.0 + 50.0 * angle.sin(), 64.0 - 50.0 * angle.cos()]
        })
        .collect();
    let square = [[6.0, 6.0], [122.0, 6.0], [122.0, 122.0], [6.0, 122.0]];
    let contours = [square.to_vec(), star];
    let mut data = String::new();
    let mut segments = Vec::new();
    for contour in &contours {
        for (index, &from) in contour.iter().enumerate() {
            let to = contour[(index + 1) % contour.len()];
            segments.push((from, to));
            for step in 0..300 {
                let t = step as f64 / 300.0;
                let command = if index == 0 && step == 0 { "M" } else { "L" };
                data += &format!(
                    "{command} {:.4} {:.4} ",
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t
                );
            }
        }
        data += "Z ";
    }
    let (scale, rotation) = ((1.1, 0.8), 20.0_f64);
    let path = NodeKind::Path(Path {
        id: NodeId::new(1),
        params: PathParamsDelegate {
            data: Deferred::value(data),
            fill_paint: Paint::solid([255, 0, 0, 255]).into(),
            stroke_enabled: Deferred::value(true),
            stroke_paint: Paint::solid([0, 0, 255, 255]).into(),
            stroke_width: Deferred::value(3.0),
            scale: Deferred::value(scale),
            rotation: Deferred::value(rotation),
            anchor: Deferred::value((64.0, 64.0)),
            ..Default::default()
        },
    });
    let pixels = render_drawable_graph(&mut renderer, vec![path], 128);

    let (sin, cos) = rotation.to_radians().sin_cos();
    let mut checked = [0; 4];
    for y in 0..128 {
        for x in 0..128 {
            let (dx, dy) = (x as f64 + 0.5 - 64.0, y as f64 + 0.5 - 64.0);
            let local = [
                64.0 + (cos * dx + sin * dy) / scale.0,
                64.0 + (-sin * dx + cos * dy) / scale.1,
            ];
            let mut inside = false;
            let mut distance = f64::MAX;
            for &(a, b) in &segments {
                if (a[1] > local[1]) != (b[1] > local[1])
                    && local[0] < (b[0] - a[0]) * (local[1] - a[1]) / (b[1] - a[1]) + a[0]
                {
                    inside = !inside;
                }
                let ab = [b[0] - a[0], b[1] - a[1]];
                let h = (((local[0] - a[0]) * ab[0] + (local[1] - a[1]) * ab[1])
                    / (ab[0] * ab[0] + ab[1] * ab[1]))
                    .clamp(0.0, 1.0);
                distance =
                    distance.min((local[0] - a[0] - ab[0] * h).hypot(local[1] - a[1] - ab[1] * h));
            }
            // Stroke covers 1.5 untransformed pixels; at 0.8x a canvas pixel is 1.25.
            let expected = if distance < 0.5 {
                [0, 0, 255, 255]
            } else if distance > 1.5 + 2.5 {
                if inside {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 0, 0]
                }
            } else {
                continue;
            };
            let in_square = local.iter().all(|value| (6.0..122.0).contains(value));
            let class = match (distance < 0.5, inside, in_square) {
                (true, _, _) => 0,
                (_, true, _) => 1,
                (_, false, true) => 2,
                (_, false, false) => 3,
            };
            checked[class] += 1;
            let offset = (y * 128 + x) * 4;
            assert_eq!(
                pixels[offset..offset + 4],
                expected,
                "pixel ({x}, {y}), inside {inside}, distance {distance:.2}"
            );
        }
    }
    // Stroked, filled, holes in the square, and outside it.
    assert!(checked.iter().all(|&count| count > 300), "{checked:?}");
}

#[test]
fn text_opacity_fades_overlapping_glyphs_as_one_layer() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = |opacity: f64| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value("A\u{338} f\u{301}".to_string()),
                font_size: Deferred::value(48.0),
                position: Deferred::value((8.0, 8.0)),
                opacity: Deferred::value(opacity),
                ..Default::default()
            },
        })]
    };
    let opaque = render_drawable_graph(&mut renderer, text(1.0), 128);
    let translucent = render_drawable_graph(&mut renderer, text(0.5), 128);
    assert!(opaque.iter().skip(3).step_by(4).any(|&a| a == 255));
    for (full, half) in opaque.iter().zip(&translucent) {
        assert!(
            (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= 1.0,
            "{full} at half opacity gave {half}"
        );
    }
}

#[test]
fn scaled_text_supersamples_paint_over_one_canvas_pixel() {
    use lumen_engine::node::{source::text::TextRenderMode, vector::paint::GradientUnits};
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let stop = |offset: f32, color: [u8; 4]| GradientStop { offset, color };
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    // A hard red-to-blue stop through the crossbar of an `H` scaled 8x, at canvas x 57.6.
    let text = vec![NodeKind::Text(Text {
        id: NodeId::new(1),
        params: TextParamsDelegate {
            content: Deferred::value("H".to_string()),
            font_size: Deferred::value(16.0),
            render_mode: TextRenderMode::Msdf.into(),
            position: Deferred::value((8.0, 8.0)),
            scale: Deferred::value((8.0, 8.0)),
            paint_supersample: Deferred::value(true),
            color: Paint::Gradient(GradientPaint {
                units: GradientUnits::UserSpace,
                start: [0.0, 0.0],
                end: [28.4, 0.0],
                stops: vec![
                    stop(0.0, red),
                    stop(0.5, red),
                    stop(0.5, blue),
                    stop(1.0, blue),
                ],
                ..Default::default()
            })
            .into(),
            ..Default::default()
        },
    })];
    let pixels = render_drawable_graph(&mut renderer, text, 128);
    let mixed_columns = (0..128)
        .filter(|&x| {
            (0..128).any(|y| {
                let pixel = &pixels[(y * 128 + x) * 4..][..4];
                pixel[3] == 255 && pixel[0] > 8 && pixel[2] > 8
            })
        })
        .count();
    assert!(
        pixels.as_chunks::<4>().0.iter().any(|pixel| *pixel == red)
            && pixels.as_chunks::<4>().0.iter().any(|pixel| *pixel == blue)
    );
    assert!(
        mixed_columns <= 2,
        "the stop is blurred over {mixed_columns} columns"
    );
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

/// A white rectangle on a transparent canvas, in pixels.
#[derive(Clone, Copy)]
struct WhiteRect {
    x: f64,
    y: f64,
    width: i64,
    height: i64,
}

/// What a test renders the rectangle through. `shadow_offset` selects `shadow`
/// (black, opacity 1) with that offset instead of `blur`.
#[derive(Clone, Copy)]
struct BlurCase {
    canvas: u32,
    rect: WhiteRect,
    radius: f64,
    shadow_offset: Option<(f64, f64)>,
    straight_alpha: bool,
}

fn render_blur_case(case: BlurCase) -> Vec<u8> {
    let (shape, convert, effect, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
    );
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            params: ShapeParamsDelegate {
                width: Deferred::value(case.rect.width),
                height: Deferred::value(case.rect.height),
                position: Deferred::value((case.rect.x, case.rect.y)),
                ..Default::default()
            },
        }),
    );
    let mut source = shape;
    if case.straight_alpha {
        graph.nodes.insert(
            convert,
            NodeKind::AlphaPremultiply(AlphaPremultiply {
                id: convert,
                params: AlphaPremultiplyParamsDelegate {
                    mode: Deferred::value("unpremultiply".to_string()),
                },
                source: PortRef::new(shape, "output".to_string()),
            }),
        );
        source = convert;
    }
    let source = PortRef::new(source, "output".to_string());
    graph.nodes.insert(
        effect,
        if let Some((offset_x, offset_y)) = case.shadow_offset {
            NodeKind::Shadow(Shadow {
                id: effect,
                params: ShadowParamsDelegate {
                    offset_x: Deferred::value(offset_x),
                    offset_y: Deferred::value(offset_y),
                    radius: Deferred::value(case.radius),
                    color: Paint::solid([0, 0, 0, 255]).into(),
                    opacity: Deferred::value(1.0),
                },
                source,
            })
        } else {
            NodeKind::Blur(Blur {
                id: effect,
                params: BlurParamsDelegate {
                    radius: Deferred::value(case.radius),
                },
                source,
            })
        },
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(effect, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = case.canvas;
    composition.render_settings.height = case.canvas;
    let Some(mut renderer) = gpu_composition_renderer() else {
        return Vec::new();
    };
    render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore)
}

/// Renders a white square on a transparent canvas through `blur` or `shadow`
/// (black, opacity 1, no offset) and returns the RGBA pixels.
fn blur_square(
    canvas: u32,
    square: (i64, f64),
    radius: f64,
    use_shadow: bool,
    straight_alpha: bool,
) -> Vec<u8> {
    render_blur_case(BlurCase {
        canvas,
        rect: WhiteRect {
            x: square.1,
            y: square.1,
            width: square.0,
            height: square.0,
        },
        radius,
        shadow_offset: use_shadow.then_some((0.0, 0.0)),
        straight_alpha,
    })
}

fn alpha_at(pixels: &[u8], canvas: u32, x: u32, y: u32) -> f32 {
    f32::from(pixels[((y * canvas + x) * 4 + 3) as usize])
}

#[test]
fn blur_fractional_radius_changes_output_smoothly() {
    // Pixel 2 texels outside the square's left edge.
    let alpha = |radius| {
        let pixels = blur_square(64, (16, 24.0), radius, false, false);
        (!pixels.is_empty()).then(|| alpha_at(&pixels, 64, 22, 32))
    };
    let (Some(a20), Some(a25), Some(a30)) = (alpha(2.0), alpha(2.5), alpha(3.0)) else {
        return;
    };
    assert!(
        a20 + 2.0 < a25 && a25 + 2.0 < a30,
        "alpha must grow smoothly with radius: 2.0 -> {a20}, 2.5 -> {a25}, 3.0 -> {a30}"
    );
}

#[test]
fn blur_supports_radii_above_32_and_preserves_energy() {
    let canvas = 256;
    let pixels = blur_square(canvas, (32, 112.0), 80.0, false, false);
    if pixels.is_empty() {
        return;
    }
    // 60 texels beyond the edge is outside the old 32 pixel reach.
    let far = alpha_at(&pixels, canvas, 112 - 60, 128);
    assert!(far > 0.0, "radius 80 must reach 60 pixels from the source");
    let center = alpha_at(&pixels, canvas, 128, 128);
    assert!(
        center > 0.0 && center < 128.0,
        "a 32 pixel square under radius 80 must be diluted: {center}"
    );
    let total: f32 = pixels
        .chunks_exact(4)
        .map(|pixel| f32::from(pixel[3]) / 255.0)
        .sum();
    let expected = 32.0 * 32.0;
    assert!(
        (total - expected).abs() / expected < 0.03,
        "blur must preserve total alpha: expected about {expected}, got {total}"
    );
}

#[test]
fn shadow_blur_matches_blur_alpha() {
    let blur = blur_square(64, (16, 24.0), 6.5, false, false);
    let shadow = blur_square(64, (16, 24.0), 6.5, true, false);
    if blur.is_empty() || shadow.is_empty() {
        return;
    }
    // Outside the square the shadow output is just the blurred alpha.
    for (x, y) in [(10, 32), (16, 20), (46, 32), (32, 48), (4, 4)] {
        let (blur, shadow) = (alpha_at(&blur, 64, x, y), alpha_at(&shadow, 64, x, y));
        assert!(
            (blur - shadow).abs() <= 1.0,
            "shadow and blur must share one kernel at ({x}, {y}): blur {blur}, shadow {shadow}"
        );
    }
    let large = blur_square(256, (32, 112.0), 80.0, true, false);
    assert!(
        alpha_at(&large, 256, 112 - 60, 128) > 0.0,
        "shadow must also support radii above 32"
    );
}

#[test]
fn blur_of_antialiased_edge_has_no_dark_fringe() {
    for straight_alpha in [false, true] {
        let pixels = blur_square(64, (16, 24.5), 5.0, false, straight_alpha);
        if pixels.is_empty() {
            return;
        }
        for pixel in pixels.as_chunks::<4>().0 {
            assert!(
                pixel[0].abs_diff(pixel[3]) <= 2 && pixel[2].abs_diff(pixel[3]) <= 2,
                "white blurred over transparent must keep rgb equal to alpha (straight source: {straight_alpha}): {pixel:?}"
            );
        }
    }
}

/// Mass and per-axis variance of the alpha along a row of the canvas.
fn row_alpha_moments(pixels: &[u8], canvas: u32, row: u32, center: u32) -> (f32, f32) {
    let mut mass = 0.0;
    let mut second_moment = 0.0;
    for x in 0..canvas {
        let alpha = alpha_at(pixels, canvas, x, row);
        let distance = x as f32 - center as f32;
        mass += alpha;
        second_moment += alpha * distance * distance;
    }
    (mass, second_moment / mass)
}

#[test]
fn blur_small_radii_match_the_box_blur_they_replaced() {
    for radius in [1u32, 2] {
        let pixels = blur_square(32, (1, 16.0), f64::from(radius), false, false);
        if pixels.is_empty() {
            return;
        }
        // A (2r + 1)^2 box blur spreads a single white pixel evenly over the box.
        let side = 2 * radius + 1;
        let box_variance = (radius * (radius + 1)) as f32 / 3.0;
        let box_center_alpha = 255.0 / (side * side) as f32;
        let (mass, variance) = row_alpha_moments(&pixels, 32, 16, 16);
        let center_alpha = alpha_at(&pixels, 32, 16, 16);
        let total: f32 = pixels
            .chunks_exact(4)
            .map(|pixel| f32::from(pixel[3]))
            .sum();
        assert!(
            (variance - box_variance).abs() <= 0.15 * box_variance,
            "radius {radius}: variance along x must match the box blur's {box_variance}, got {variance}"
        );
        assert!(
            (total - 255.0).abs() <= 8.0,
            "radius {radius}: blur must preserve the impulse's alpha, got {total}"
        );
        assert!(
            center_alpha > box_center_alpha && center_alpha < 3.0 * box_center_alpha,
            "radius {radius}: centre alpha {center_alpha} must stay near the box blur's {box_center_alpha} (row mass {mass})"
        );
    }
}

#[test]
fn blur_has_no_pop_where_the_reduced_levels_take_over() {
    // The reduced levels fade in over sigma 18-22 (radius ~30.7-37.6) and 72-88
    // (radius ~124-152), and are fully in charge past them.
    for (below, above) in [(34.0, 34.3), (137.5, 138.5)] {
        let canvas = 512;
        let a = blur_square(canvas, (128, 192.0), below, false, false);
        let b = blur_square(canvas, (128, 192.0), above, false, false);
        if a.is_empty() || b.is_empty() {
            return;
        }
        let worst = a.iter().zip(&b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(
            worst <= 3,
            "radius {below} -> {above} must not visibly change the output, got a difference of {worst}"
        );
    }
}

#[test]
fn blur_keeps_canvas_edge_features_across_level_switches() {
    // A stripe touching the canvas edge is extended outward by the clamped edge.
    // Reduced levels must keep that extension instead of clamping to the first
    // block average, so the stripe's alpha does not jump when a level takes over.
    let canvas = 512;
    for (width, radii) in [
        (1, [34.14, 34.15]),
        (4, [138.06, 138.07]),
        (1, [123.5, 152.5]),
    ] {
        for x in [0.0, f64::from(canvas) - width as f64] {
            let alphas: Vec<_> = radii
                .iter()
                .map(|&radius| {
                    let rect = WhiteRect {
                        x,
                        y: 0.0,
                        width,
                        height: i64::from(canvas),
                    };
                    let pixels = render_blur_case(BlurCase {
                        canvas,
                        rect,
                        radius,
                        shadow_offset: None,
                        straight_alpha: false,
                    });
                    (!pixels.is_empty()).then(|| alpha_at(&pixels, canvas, x as u32, 256))
                })
                .collect();
            let [Some(a), Some(b)] = alphas[..] else {
                return;
            };
            assert!(
                a > 60.0 && (a - b).abs() <= 6.0,
                "{width} px stripe at x={x}: radius {} -> {} must keep its edge value, got {a} -> {b}",
                radii[0],
                radii[1]
            );
        }
    }
}

#[test]
fn shadow_keeps_content_shifted_outside_the_canvas_across_level_switches() {
    // The shadow of a rectangle shifted fully off the canvas still bleeds back in
    // through the blur, at every level and across the switches between them.
    let canvas = 512;
    let alpha = |rect: WhiteRect, offset: (f64, f64), radius: f64, at: (u32, u32)| {
        let pixels = render_blur_case(BlurCase {
            canvas,
            rect,
            radius,
            shadow_offset: Some(offset),
            straight_alpha: false,
        });
        (!pixels.is_empty()).then(|| alpha_at(&pixels, canvas, at.0, at.1))
    };
    let across_x = WhiteRect {
        x: 100.0,
        y: 200.0,
        width: 128,
        height: 128,
    };
    let across_y = WhiteRect {
        x: 200.0,
        y: 100.0,
        width: 128,
        height: 128,
    };
    // Both rectangles land at -130..-2 along the shifted axis. Each entry is a
    // radius followed by the one just past a level switch, if any.
    let edge_alphas = |radius: f64| {
        Some((
            alpha(across_x, (-230.0, 0.0), radius, (0, 264))?,
            alpha(across_y, (0.0, -230.0), radius, (264, 0))?,
        ))
    };
    for radius in [10.0, 80.0, 200.0, 34.14, 34.15, 138.06, 138.07] {
        let Some((horizontal, vertical)) = edge_alphas(radius) else {
            return;
        };
        assert!(
            horizontal > 10.0 && vertical > 10.0,
            "radius {radius}: shadow shifted off the canvas must still reach it, got x {horizontal}, y {vertical}"
        );
    }
    for (below, above) in [(34.14, 34.15), (138.06, 138.07)] {
        let (Some(before), Some(after)) = (edge_alphas(below), edge_alphas(above)) else {
            return;
        };
        assert!(
            (before.0 - after.0).abs() <= 6.0 && (before.1 - after.1).abs() <= 6.0,
            "radius {below} -> {above} must not pop: {before:?} -> {after:?}"
        );
    }
}
