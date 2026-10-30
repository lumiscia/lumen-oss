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
        ClipProps, Deferred, NodeId, NodeKind, PortRef, ShaderUniforms,
        compositing::{
            boolean::{Boolean, BooleanOperation, BooleanParamsDelegate},
            clip::ClipParamsDelegate,
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
            wgsl_shader::{WgslShader, WgslShaderParamsDelegate},
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
use serde_json::json;
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
    // The transform places the plate on the canvas but stores only the pixels it covers.
    assert_eq!(
        transformed_raster.domain.display_size(),
        lumen_gpu::Size::new(1920, 1080)
    );
    assert_eq!(
        transformed_raster.domain.data_rect,
        lumen_gpu::RectI::new(0, 0, 323, 183)
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
            params: MemoParamsDelegate::default(),
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
    assert_eq!(compiled.compiled_nodes.len(), 2);
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

#[test]
fn small_shape_compiles_to_a_texture_and_dispatch_around_its_bounds() {
    let shape = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            params: ShapeParamsDelegate {
                width: Deferred::value(40),
                height: Deferred::value(20),
                position: Deferred::value((100.0, 200.0)),
                stroke_enabled: Deferred::value(true),
                stroke_width: Deferred::value(2.0),
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
    composition.render_settings.width = 1920;
    composition.render_settings.height = 1080;

    let compiled = CompileContext::new(&composition).compile().unwrap();
    let raster = compiled
        .node_outputs
        .get(&PortRef::new(shape, "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(shape, "output")
        .unwrap();
    let data = raster.domain.data_rect;
    let texture = compiled
        .plan
        .textures()
        .iter()
        .find(|texture| texture.id == raster.texture)
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
        raster.domain.display_size(),
        lumen_gpu::Size::new(1920, 1080)
    );
    assert!(data.contains(lumen_gpu::RectI::new(99, 199, 42, 22)));
    assert!(data.width <= 48 && data.height <= 28, "{data:?}");
    assert_eq!(texture.desc.domain.storage_size, data.size());
    assert_eq!(
        dispatch,
        lumen_gpu::ComputeDispatch::Direct(lumen_gpu::Dispatch {
            x: data.width.div_ceil(8),
            y: data.height.div_ceil(8),
            z: 1,
        })
    );
    assert_eq!(
        compiled.output.domain.storage_size,
        lumen_gpu::Size::new(1920, 1080)
    );
}

/// Text drawn in a font whose glyphs are `scale` times the size of Roboto's, held back until the
/// media store resolves it.
#[derive(Debug, Clone)]
struct ScaledFontStore {
    family: &'static str,
    data: Vec<u8>,
}

impl ScaledFontStore {
    fn new(family: &'static str, scale: u16) -> Self {
        assert_eq!(family.len(), "Roboto".len());
        let mut data = lumen_text::ROBOTO_REGULAR_BYTES.to_vec();
        let utf16 = |name: &str| {
            name.encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>()
        };
        for (old, new) in [
            (b"Roboto".to_vec(), family.as_bytes().to_vec()),
            (utf16("Roboto"), utf16(family)),
        ] {
            for offset in 0..=data.len() - old.len() {
                if data[offset..offset + old.len()] == old[..] {
                    data[offset..offset + old.len()].copy_from_slice(&new);
                }
            }
        }
        let table_count = u16::from_be_bytes(data[4..6].try_into().unwrap()) as usize;
        let head = data[12..12 + table_count * 16]
            .as_chunks::<16>()
            .0
            .iter()
            .find(|table| &table[..4] == b"head")
            .unwrap();
        let units_offset = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize + 18;
        let units = u16::from_be_bytes(data[units_offset..units_offset + 2].try_into().unwrap());
        data[units_offset..units_offset + 2].copy_from_slice(&(units / scale).to_be_bytes());
        Self { family, data }
    }

    fn composition(&self, line_height: f64, bounded: bool) -> Composition {
        let text = NodeId::new(1);
        let output = NodeId::new(2);
        let mut graph = Graph::new();
        graph.nodes.insert(
            text,
            NodeKind::Text(Text {
                id: text,
                params: TextParamsDelegate {
                    content: Deferred::value("WWW".to_string()),
                    font_family: Deferred::value(self.family.to_string()),
                    font_size: if bounded {
                        Deferred::value(20.0)
                    } else {
                        Deferred::Expr(Expression::parse("20").unwrap())
                    },
                    line_height: Deferred::value(line_height),
                    position: Deferred::value((10.0, 90.0)),
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
        composition.render_settings.width = 640;
        composition.render_settings.height = 360;
        composition
    }
}

impl lumen_engine::media::FontResolver for ScaledFontStore {
    fn id(&self) -> &str {
        self.family
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(vec![self.data.clone()])
    }
}

impl MediaStore for ScaledFontStore {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }

    fn font_resolver_id(&self, family: &str) -> Option<String> {
        (family == self.family).then(|| self.family.to_string())
    }

    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        (family == self.family).then(|| Box::new(self.clone()) as _)
    }
}

/// Renders text that is bounded to its ink, after compiling without the media store when
/// `compile_first` is set, and the same text at full frame. Returns whether they are the same
/// pixels and whether the first was bounded.
fn bounded_text_matches_full_frame(
    store: &ScaledFontStore,
    line_height: f64,
    compile_first: bool,
) -> Option<(bool, bool)> {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let mut renderer = gpu_composition_renderer()?;
    let composition = store.composition(line_height, true);
    if compile_first {
        renderer.compile(&composition).unwrap();
    } else {
        renderer
            .compile_with_media(
                &composition,
                store,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
    }
    let pixels = render_rgba8(&mut renderer, &composition, 0, store);
    let text = PortRef::new(NodeId::new(1), "output".to_string());
    let bounded = !renderer.compiled().unwrap().node_outputs[&text]
        .clone()
        .into_raster(text.id, "output")
        .unwrap()
        .domain
        .is_full_frame();

    let mut reference = gpu_composition_renderer()?;
    let expected = render_rgba8(
        &mut reference,
        &store.composition(line_height, false),
        0,
        store,
    );
    Some((pixels == expected, bounded))
}

#[test]
fn bounded_text_holds_the_ink_of_the_font_it_is_drawn_in() {
    // A font that only the render's media store resolves, wider than the fallback.
    let wide = ScaledFontStore::new("WideIn", 2);
    if let Some((same, bounded)) = bounded_text_matches_full_frame(&wide, 1.2, true) {
        assert!(same, "text measured with the fallback font was clipped");
        assert!(bounded, "text in a resolved font should be bounded");
    }
    // Ink eight times the em tall, far beyond a margin measured in em from the line box.
    let tall = ScaledFontStore::new("TallIn", 8);
    if let Some((same, bounded)) = bounded_text_matches_full_frame(&tall, 1.0, false) {
        assert!(same, "text with ink outside its line box was clipped");
        assert!(bounded, "text in a resolved font should be bounded");
    }
}

impl ScaledFontStore {
    /// The same font declaring `weight` in its OS/2 table.
    fn with_weight(mut self, weight: u16) -> Self {
        let table_count = u16::from_be_bytes(self.data[4..6].try_into().unwrap()) as usize;
        let os2 = self.data[12..12 + table_count * 16]
            .as_chunks::<16>()
            .0
            .iter()
            .find(|table| &table[..4] == b"OS/2")
            .unwrap();
        let offset = u32::from_be_bytes(os2[8..12].try_into().unwrap()) as usize + 4;
        self.data[offset..offset + 2].copy_from_slice(&weight.to_be_bytes());
        self
    }
}

impl ScaledFontStore {
    /// The same font data, resolved under `family`.
    fn resolved_as(mut self, family: &'static str) -> Self {
        self.family = family;
        self
    }
}

#[test]
fn bounded_text_is_remeasured_after_binding_loads_a_font() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let text = NodeId::new(1);
    let loader = NodeId::new(2);
    let merge = NodeId::new(3);
    let output = NodeId::new(4);
    let composition = |bounded: bool| {
        let mut graph = Graph::new();
        graph.nodes.insert(
            text,
            NodeKind::Text(Text {
                id: text,
                params: TextParamsDelegate {
                    content: Deferred::value("WWWWWWWWWW".to_string()),
                    font_family: Deferred::value("Roboto".to_string()),
                    font_size: if bounded {
                        Deferred::value(20.0)
                    } else {
                        Deferred::Expr(Expression::parse("20").unwrap())
                    },
                    font_weight: Deferred::value(700),
                    position: Deferred::value((5.0, 40.0)),
                    render_mode: lumen_engine::node::source::text::TextRenderMode::Raster.into(),
                    ..Default::default()
                },
            }),
        );
        // Its font family is only known when it is bound, and the font it loads then is a
        // wider bold Roboto, which the text above is drawn in.
        graph.nodes.insert(
            loader,
            NodeKind::Text(Text {
                id: loader,
                params: TextParamsDelegate {
                    content: Deferred::value("W".to_string()),
                    font_family: Deferred::Expr(Expression::parse("\"Loader\"").unwrap()),
                    position: Deferred::value((0.0, 400.0)),
                    render_mode: lumen_engine::node::source::text::TextRenderMode::Raster.into(),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            merge,
            NodeKind::RasterMultiMerge(RasterMultiMerge {
                id: merge,
                layers: vec![
                    PortRef::new(loader, "output".to_string()),
                    PortRef::new(text, "output".to_string()),
                ],
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
        let mut composition = test_composition(graph);
        composition.render_settings.width = 256;
        composition.render_settings.height = 192;
        composition
    };
    let store = ScaledFontStore::new("Roboto", 2)
        .with_weight(700)
        .resolved_as("Loader");
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let pixels = render_rgba8(&mut renderer, &composition(true), 0, &store);

    // The font is shared by every renderer, so the reference sees it loaded from the start.
    let mut reference = gpu_composition_renderer().unwrap();
    let expected = render_rgba8(&mut reference, &composition(false), 0, &store);
    assert!(
        pixels == expected,
        "the first frame was clipped to bounds measured before binding loaded a font"
    );
}

#[test]
fn bounded_text_is_remeasured_after_another_renderer_loads_a_font() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let text_composition = |bounded: bool| {
        let text = NodeId::new(1);
        let output = NodeId::new(2);
        let mut graph = Graph::new();
        graph.nodes.insert(
            text,
            NodeKind::Text(Text {
                id: text,
                params: TextParamsDelegate {
                    content: Deferred::value("WWWWWWWWWW".to_string()),
                    font_size: if bounded {
                        Deferred::value(20.0)
                    } else {
                        Deferred::Expr(Expression::parse("20").unwrap())
                    },
                    font_weight: Deferred::value(700),
                    position: Deferred::value((5.0, 40.0)),
                    render_mode: lumen_engine::node::source::text::TextRenderMode::Raster.into(),
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
        composition.render_settings.width = 256;
        composition.render_settings.height = 192;
        composition
    };
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = text_composition(true);
    render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);

    // A wider bold Roboto, loaded into the text system that every renderer shares.
    let wide = ScaledFontStore::new("Roboto", 2).with_weight(700);
    let mut loader = gpu_composition_renderer().unwrap();
    render_rgba8(&mut loader, &composition, 0, &wide);

    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    let mut reference = gpu_composition_renderer().unwrap();
    let expected = render_rgba8(
        &mut reference,
        &text_composition(false),
        0,
        &EmptyMediaStore,
    );
    assert!(
        pixels == expected,
        "text was clipped to bounds measured before a font was loaded"
    );
}

#[derive(Clone, Copy)]
enum TransformParamKind {
    Literal,
    /// An expression of the frame, whose bounds are held over the timeline.
    Animated,
    /// An expression that reads another node, so that the transform fills the canvas.
    Unbounded,
}

impl TransformParamKind {
    fn param(self, name: &str, value: f64) -> Deferred<f64> {
        match self {
            Self::Literal => Deferred::value(value),
            Self::Animated => {
                Deferred::Expr(Expression::parse(&format!("{value:.0} + 0 * frame")).unwrap())
            }
            Self::Unbounded => {
                Deferred::Expr(Expression::parse(&format!("node(99, \"{name}\")")).unwrap())
            }
        }
    }
}

/// Renders a small shape through a transform with the given rotation and scale.
fn render_transformed_small_shape(
    kind: TransformParamKind,
    rotate: f64,
    scale: f64,
    pivot: Option<f64>,
    position: (f64, f64),
) -> Option<Vec<u8>> {
    let pivot_params = |kind: TransformParamKind| match pivot {
        Some(pivot) => (
            Deferred::value(true),
            kind.param("pivot_x", pivot),
            kind.param("pivot_y", pivot),
        ),
        None => (
            Deferred::value(false),
            Deferred::value(0.0),
            Deferred::value(0.0),
        ),
    };
    let (custom_pivot, pivot_x, pivot_y) = pivot_params(kind);
    let mut renderer = gpu_composition_renderer()?;
    let shape = NodeId::new(1);
    let transform = NodeId::new(2);
    let output = NodeId::new(3);
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            params: ShapeParamsDelegate {
                position: Deferred::value(position),
                width: Deferred::value(14),
                height: Deferred::value(11),
                fill_paint: Paint::solid([255, 100, 0, 200]).into(),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        transform,
        NodeKind::Transform(Transform {
            id: transform,
            source: PortRef::new(shape, "output".to_string()),
            params: TransformParamsDelegate {
                rotate: kind.param("rotate", rotate),
                scale_x: kind.param("scale_x", scale),
                scale_y: kind.param("scale_y", scale),
                custom_pivot,
                pivot_x,
                pivot_y,
                ..Default::default()
            },
        }),
    );
    // The values the unbounded reference reads.
    graph.nodes.insert(
        NodeId::new(99),
        NodeKind::Transform(Transform {
            id: NodeId::new(99),
            source: PortRef::new(shape, "output".to_string()),
            params: TransformParamsDelegate {
                rotate: Deferred::value(rotate),
                scale_x: Deferred::value(scale),
                scale_y: Deferred::value(scale),
                custom_pivot: pivot_params(TransformParamKind::Literal).0,
                pivot_x: pivot_params(TransformParamKind::Literal).1,
                pivot_y: pivot_params(TransformParamKind::Literal).2,
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(transform, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.render_settings.width = 256;
    composition.render_settings.height = 192;
    composition.timeline.duration_frames = 2;
    Some(render_rgba8(
        &mut renderer,
        &composition,
        0,
        &EmptyMediaStore,
    ))
}

#[test]
fn transform_bounds_never_clip_what_the_shader_draws() {
    // A rotation too large for the shader's single-precision angle to be bounded, and a scale
    // that overflows single precision.
    // A pivot far enough away that single precision cannot place the shape about it.
    for (rotate, scale, pivot, position) in [
        (1e9, 1.0, None, (40.0, 45.0)),
        (4.5e4, 1.0, None, (40.0, 45.0)),
        (0.0, 1e40, None, (40.0, 45.0)),
        (0.0, 1e300, None, (40.0, 45.0)),
        (0.0, 1.0, Some(1e10), (0.0, 0.0)),
        (0.0, 1.0, Some(1e7), (0.0, 0.0)),
        (30.0, 1.0, Some(1e6), (0.0, 0.0)),
    ] {
        for kind in [TransformParamKind::Literal, TransformParamKind::Animated] {
            let Some(pixels) = render_transformed_small_shape(kind, rotate, scale, pivot, position)
            else {
                return;
            };
            let expected = render_transformed_small_shape(
                TransformParamKind::Unbounded,
                rotate,
                scale,
                pivot,
                position,
            )
            .unwrap();
            assert!(
                pixels == expected,
                "transform with rotate {rotate}, scale {scale} and pivot {pivot:?} was clipped"
            );
        }
    }
}

fn moving_shape_composition(
    position: &str,
    width: Deferred<i64>,
    fps: f32,
    duration_frames: u32,
) -> Composition {
    let shape = NodeId::new(1);
    let output = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            params: ShapeParamsDelegate {
                position: Deferred::Expr(Expression::parse(position).unwrap()),
                width,
                height: Deferred::value(8),
                fill_paint: Paint::solid([255, 100, 0, 255]).into(),
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
    composition.render_settings.width = 128;
    composition.render_settings.height = 64;
    composition.timeline = TimelineSettings {
        fps,
        duration_frames,
    };
    composition
}

fn pixel_alpha(pixels: &[u8], width: u32, x: u32, y: u32) -> u8 {
    pixels[(y * width + x) as usize * 4 + 3]
}

#[test]
fn animated_bounds_follow_timeline_edits() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut composition =
        moving_shape_composition("vec2(10 + frame * 10, 20)", Deferred::value(8), 24.0, 2);
    render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);

    composition.timeline.duration_frames = 10;
    let pixels = render_rgba8(&mut renderer, &composition, 8, &EmptyMediaStore);
    assert!(
        pixel_alpha(&pixels, 128, 94, 24) > 0,
        "duration edit clipped the shape"
    );

    let mut composition =
        moving_shape_composition("vec2(10 + time * 100, 20)", Deferred::value(8), 24.0, 10);
    renderer.release_resources();
    render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);

    composition.timeline.fps = 12.0;
    let pixels = render_rgba8(&mut renderer, &composition, 8, &EmptyMediaStore);
    assert!(
        pixel_alpha(&pixels, 128, 80, 24) > 0,
        "fps edit clipped the shape"
    );
}

#[test]
fn animated_bounds_follow_referenced_node_edits() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let referenced = NodeId::new(99);
    let mut composition = moving_shape_composition(
        "vec2(10, 20)",
        Deferred::Expr(Expression::parse("node(99, \"width\")").unwrap()),
        24.0,
        2,
    );
    composition.graph.nodes.insert(
        referenced,
        NodeKind::Shape(Shape {
            id: referenced,
            params: ShapeParamsDelegate {
                width: Deferred::value(8),
                ..Default::default()
            },
        }),
    );
    render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);

    let Some(NodeKind::Shape(shape)) = composition.graph.nodes.get_mut(&referenced) else {
        unreachable!("the referenced node is a shape");
    };
    shape.params.width = Deferred::value(100);
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    assert!(
        pixel_alpha(&pixels, 128, 100, 24) > 0,
        "referenced edit clipped the shape"
    );
}

#[test]
fn standalone_compile_keeps_animated_sources_full_frame_for_fractional_binds() {
    let composition = moving_shape_composition(
        "vec2(20 + 100 * sin(frame * 3.141592653589793), 50)",
        Deferred::value(8),
        24.0,
        2,
    );
    let shape = PortRef::new(NodeId::new(1), "output".to_string());
    let shape_domain = |compiled: &lumen_engine::gpu::CompiledComposition| {
        compiled.node_outputs[&shape]
            .clone()
            .into_raster(shape.id, "output")
            .unwrap()
            .domain
    };

    let standalone = CompileContext::new(&composition).compile().unwrap();
    assert!(shape_domain(&standalone).is_full_frame());

    if let Some(mut renderer) = gpu_composition_renderer() {
        renderer.compile(&composition).unwrap();
        assert!(!shape_domain(renderer.compiled().unwrap()).is_full_frame());
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
            sources: vec![PortRef::new(straight, "output".to_string())],
            ..WgslShader::default()
        }),
    );
    assert_pixels(
        with_output(graph, shader),
        [128, 0, 0, 128],
        "default WGSL shader with zero tint must premultiply a straight input",
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
    read_texture_rgba8(
        gpu,
        gpu.texture(raster.texture).unwrap(),
        raster.domain.storage_size,
    )
}

fn read_texture_rgba8(
    gpu: &lumen_gpu::Renderer,
    texture: &lumen_gpu::wgpu::Texture,
    size: lumen_gpu::Size,
) -> Vec<u8> {
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
        texture.as_image_copy(),
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
    render_drawable_frame(renderer, nodes, size, 0, &EmptyMediaStore)
}

fn render_drawable_frame(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    nodes: Vec<NodeKind>,
    size: u32,
    frame: u32,
    media: &impl MediaStore,
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
            media,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .unwrap();
    render_rgba8(renderer, &composition, frame, media)
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

fn expression<T>(source: &str) -> Deferred<T> {
    Deferred::Expr(Expression::parse(source).unwrap())
}

/// Averages renders of the same size, as the shutter's taps are averaged.
fn average_renders(renders: &[Vec<u8>]) -> Vec<u8> {
    (0..renders[0].len())
        .map(|index| {
            let sum: f64 = renders.iter().map(|render| f64::from(render[index])).sum();
            (sum / renders.len() as f64).round() as u8
        })
        .collect()
}

/// Frames at the middle of each of `taps` equal parts of a shutter of `angle` degrees
/// centred on `frame`.
fn tap_frames(frame: u32, angle: f64, taps: usize) -> Vec<f64> {
    (0..taps)
        .map(|tap| {
            let s = (2 * tap + 1) as f64 / taps as f64 - 1.0;
            f64::from(frame) + s * angle / 720.0
        })
        .collect()
}

fn total_alpha(pixels: &[u8]) -> u64 {
    pixels
        .iter()
        .skip(3)
        .step_by(4)
        .map(|&a| u64::from(a))
        .sum()
}

#[test]
fn motion_blur_on_still_layers_changes_nothing() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let media = TestMediaStore::image("plate", 16, 16);
    // An expression that holds still is evaluated over the shutter like a moving one.
    let layers = |motion_blur: f64| {
        vec![
            vec![NodeKind::Shape(Shape {
                id: NodeId::new(1),
                params: ShapeParamsDelegate {
                    width: Deferred::value(30),
                    height: Deferred::value(20),
                    position: expression("vec2(30.5, 40)"),
                    rotation: Deferred::value(20.0),
                    stroke_enabled: Deferred::value(true),
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
            })],
            vec![NodeKind::Path(Path {
                id: NodeId::new(1),
                params: PathParamsDelegate {
                    data: Deferred::value("M 5 5 Q 40 0 45 40 L 5 45 Z".to_string()),
                    position: expression("vec2(20, 10 + 0 * frame)"),
                    scale: Deferred::value((1.5, 0.75)),
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
            })],
            vec![NodeKind::Text(Text {
                id: NodeId::new(1),
                params: TextParamsDelegate {
                    content: Deferred::value("Lumen".to_string()),
                    font_size: Deferred::value(24.0),
                    render_mode: TextRenderMode::Msdf.into(),
                    position: expression("vec2(10, 30)"),
                    rotation: Deferred::value(10.0),
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
            })],
            vec![
                NodeKind::MediaIn(MediaIn {
                    id: NodeId::new(1),
                    params: MediaInParamsDelegate {
                        source: Deferred::value("plate".to_string()),
                        ..Default::default()
                    },
                }),
                NodeKind::Transform(Transform {
                    id: NodeId::new(2),
                    params: TransformParamsDelegate {
                        translate_x: expression("20 + 0 * frame"),
                        rotate: Deferred::value(30.0),
                        motion_blur: Deferred::value(motion_blur),
                        ..Default::default()
                    },
                    source: PortRef::new(NodeId::new(1), "output".to_string()),
                }),
            ],
        ]
    };
    for (still, blurred) in layers(0.0).into_iter().zip(layers(180.0)) {
        let still = render_drawable_frame(&mut renderer, still, 64, 3, &media);
        let blurred = render_drawable_frame(&mut renderer, blurred, 64, 3, &media);
        assert!(total_alpha(&still) > 0);
        assert_eq!(blurred, still);
    }
}

#[test]
fn moving_rectangle_smear_matches_averaged_subframe_renders() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let rectangle = |position: Deferred<(f64, f64)>, motion_blur: f64| {
        vec![NodeKind::Shape(Shape {
            id: NodeId::new(1),
            params: ShapeParamsDelegate {
                width: Deferred::value(20),
                height: Deferred::value(12),
                position,
                rotation: Deferred::value(15.0),
                anchor: Deferred::value((10.0, 6.0)),
                motion_blur: Deferred::value(motion_blur),
                ..Default::default()
            },
        })]
    };
    let moving = || expression("vec2(10 + frame * 31, 30)");
    // Over a 180 degree shutter the rectangle travels 15.5 pixels, so it takes 16 taps.
    let blurred = render_drawable_frame(
        &mut renderer,
        rectangle(moving(), 180.0),
        96,
        2,
        &EmptyMediaStore,
    );
    let still = render_drawable_frame(
        &mut renderer,
        rectangle(moving(), 0.0),
        96,
        2,
        &EmptyMediaStore,
    );
    let taps: Vec<_> = tap_frames(2, 180.0, 16)
        .into_iter()
        .map(|frame| {
            let position = Deferred::value((10.0 + frame * 31.0, 30.0));
            render_drawable_frame(
                &mut renderer,
                rectangle(position, 0.0),
                96,
                2,
                &EmptyMediaStore,
            )
        })
        .collect();
    let expected = average_renders(&taps);
    let difference = max_channel_difference(&blurred, &expected);
    eprintln!(
        "smear: max difference {difference}, partially covered {} vs {} still",
        partially_covered(&blurred),
        partially_covered(&still)
    );
    assert!(partially_covered(&blurred) > 4 * partially_covered(&still));
    assert!(difference <= 2, "max difference {difference}");
}

#[test]
fn rotation_smears_along_its_arc() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // An 8 pixel square 32 pixels above the canvas centre, turning about the centre by 90
    // degrees a frame. A 360 degree shutter at frame 1 sweeps it from 45 to 135 degrees.
    let square = NodeKind::Shape(Shape {
        id: NodeId::new(1),
        params: ShapeParamsDelegate {
            width: Deferred::value(8),
            height: Deferred::value(8),
            position: Deferred::value((60.0, 28.0)),
            anchor: Deferred::value((4.0, 36.0)),
            rotation: expression("frame * 90"),
            motion_blur: Deferred::value(360.0),
            ..Default::default()
        },
    });
    let pixels = render_drawable_frame(&mut renderer, vec![square], 128, 1, &EmptyMediaStore);
    let alpha_at = |degrees: f64, radius: f64| {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let (x, y) = (64.0 + radius * sin, 64.0 - radius * cos);
        pixels[(y as usize * 128 + x as usize) * 4 + 3]
    };
    for degrees in [55.0, 90.0, 125.0] {
        let alpha = alpha_at(degrees, 32.0);
        assert!(
            alpha > 0 && alpha < 128,
            "alpha {alpha} at {degrees} degrees"
        );
    }
    // The chord between the sweep's ends lies 9.4 pixels inside the arc.
    assert_eq!(alpha_at(90.0, 22.0), 0);
    assert_eq!(alpha_at(20.0, 32.0), 0);
    assert_eq!(alpha_at(160.0, 32.0), 0);
}

#[test]
fn blurred_image_through_transform_matches_averaged_subframe_renders() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let media = TestMediaStore::image("plate", 16, 16);
    let image = |translate_x: Deferred<f64>, motion_blur: f64| {
        vec![
            NodeKind::MediaIn(MediaIn {
                id: NodeId::new(1),
                params: MediaInParamsDelegate {
                    source: Deferred::value("plate".to_string()),
                    ..Default::default()
                },
            }),
            NodeKind::Transform(Transform {
                id: NodeId::new(2),
                params: TransformParamsDelegate {
                    translate_x,
                    translate_y: Deferred::value(20.0),
                    rotate: Deferred::value(10.0),
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
                source: PortRef::new(NodeId::new(1), "output".to_string()),
            }),
        ]
    };
    // Over a 180 degree shutter the image travels 9.5 pixels, so it takes 10 taps.
    let blurred = render_drawable_frame(
        &mut renderer,
        image(expression("10 + frame * 19"), 180.0),
        64,
        1,
        &media,
    );
    let still = render_drawable_frame(
        &mut renderer,
        image(expression("10 + frame * 19"), 0.0),
        64,
        1,
        &media,
    );
    let taps: Vec<_> = tap_frames(1, 180.0, 10)
        .into_iter()
        .map(|frame| {
            render_drawable_frame(
                &mut renderer,
                image(Deferred::value(10.0 + frame * 19.0), 0.0),
                64,
                1,
                &media,
            )
        })
        .collect();
    let expected = average_renders(&taps);
    let difference = max_channel_difference(&blurred, &expected);
    eprintln!(
        "image smear: max difference {difference}, partially covered {} vs {} still",
        partially_covered(&blurred),
        partially_covered(&still)
    );
    assert!(partially_covered(&blurred) > 2 * partially_covered(&still));
    assert!(difference <= 2, "max difference {difference}");
}

#[test]
fn blurred_text_keeps_its_coverage_where_glyph_smears_cross() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        let text = |motion_blur: f64| {
            vec![NodeKind::Text(Text {
                id: NodeId::new(1),
                params: TextParamsDelegate {
                    content: Deferred::value("Lumen".to_string()),
                    font_size: Deferred::value(32.0),
                    render_mode: mode.into(),
                    position: expression("vec2(frame * 40 - 20, 40)"),
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
            })]
        };
        // Each glyph smears 40 pixels, across its neighbours.
        let still = render_drawable_frame(&mut renderer, text(0.0), 192, 2, &EmptyMediaStore);
        let blurred = render_drawable_frame(&mut renderer, text(360.0), 192, 2, &EmptyMediaStore);
        let (still_alpha, blurred_alpha) = (total_alpha(&still), total_alpha(&blurred));
        eprintln!("{mode:?} text: total alpha {blurred_alpha} blurred vs {still_alpha} still");
        assert!(partially_covered(&blurred) > 2 * partially_covered(&still));
        assert!(still_alpha.abs_diff(blurred_alpha) * 50 < still_alpha);
    }
}

#[test]
fn blur_keeps_taps_around_a_frame_whose_scale_is_zero() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // The scale is zero at frame 1 and not at any other time of a 360 degree shutter.
    let layers = |scale: Deferred<(f64, f64)>, motion_blur: f64| {
        vec![
            vec![NodeKind::Shape(Shape {
                id: NodeId::new(1),
                params: ShapeParamsDelegate {
                    width: Deferred::value(40),
                    height: Deferred::value(20),
                    position: Deferred::value((20.0, 30.0)),
                    scale: scale.clone(),
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
            })],
            vec![NodeKind::Path(Path {
                id: NodeId::new(1),
                params: PathParamsDelegate {
                    data: Deferred::value("M 0 0 L 40 0 L 40 20 L 0 20 Z".to_string()),
                    position: Deferred::value((20.0, 30.0)),
                    scale: scale.clone(),
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
            })],
            vec![NodeKind::Path(Path {
                id: NodeId::new(1),
                params: PathParamsDelegate {
                    data: Deferred::value("M 0 0 C 40 0 40 40 0 40 Z".to_string()),
                    position: Deferred::value((20.0, 30.0)),
                    scale,
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
            })],
        ]
    };
    let text = |scale: Deferred<(f64, f64)>, motion_blur: f64| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value("Lumen".to_string()),
                font_size: Deferred::value(32.0),
                render_mode: TextRenderMode::Msdf.into(),
                position: Deferred::value((10.0, 30.0)),
                scale,
                motion_blur: Deferred::value(motion_blur),
                ..Default::default()
            },
        })]
    };
    let scale = || expression::<(f64, f64)>("vec2(frame - 1, 1)");
    let cases: Vec<_> = layers(scale(), 360.0)
        .into_iter()
        .zip(["shape", "rectangle path", "curved path"])
        .chain([(text(scale(), 360.0), "text")])
        .collect();
    let mut still_cases = layers(scale(), 0.0);
    still_cases.push(text(scale(), 0.0));
    let at = |frame: f64| Deferred::value((frame - 1.0, 1.0));
    for (index, (blurred, name)) in cases.into_iter().enumerate() {
        let blurred = render_drawable_frame(&mut renderer, blurred, 96, 1, &EmptyMediaStore);
        let taps: Vec<_> = tap_frames(1, 360.0, 20)
            .into_iter()
            .map(|frame| {
                let layer = if index < 3 {
                    layers(at(frame), 0.0).swap_remove(index)
                } else {
                    text(at(frame), 0.0)
                };
                render_drawable_frame(&mut renderer, layer, 96, 1, &EmptyMediaStore)
            })
            .collect();
        let (blurred, expected) = (total_alpha(&blurred), total_alpha(&average_renders(&taps)));
        eprintln!("{name}: total alpha {blurred} vs {expected} from the taps");
        assert!(blurred > 0, "{name} is hidden");
        assert!(
            blurred.abs_diff(expected) * 10 < expected,
            "{name}: {blurred} vs {expected}"
        );
    }
}

#[test]
fn transform_blur_averages_straight_alpha_premultiplied() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let convert = |id: u64, source: u64, mode: &str| {
        NodeKind::AlphaPremultiply(AlphaPremultiply {
            id: NodeId::new(id),
            params: AlphaPremultiplyParamsDelegate {
                mode: Deferred::value(mode.to_string()),
            },
            source: PortRef::new(NodeId::new(source), "output".to_string()),
        })
    };
    // Half transparent red slides across the canvas, so some pixels see it under some taps.
    let pixels = render_drawable_frame(
        &mut renderer,
        vec![
            solid(NodeId::new(1), [255, 0, 0, 128]),
            convert(2, 1, "unpremultiply"),
            NodeKind::Transform(Transform {
                id: NodeId::new(3),
                params: TransformParamsDelegate {
                    translate_x: expression("frame * 20"),
                    motion_blur: Deferred::value(360.0),
                    sampling: TransformSampling::Nearest.into(),
                    ..Default::default()
                },
                source: PortRef::new(NodeId::new(2), "output".to_string()),
            }),
            convert(4, 3, "premultiply"),
        ],
        64,
        1,
        &EmptyMediaStore,
    );
    let mut partial = 0;
    for pixel in pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] > 0)
    {
        // The colour stays red, so premultiplied it equals the alpha.
        assert!(
            pixel[0].abs_diff(pixel[3]) <= 1 && pixel[1] == 0,
            "{pixel:?}"
        );
        partial += usize::from(pixel[3] < 127);
    }
    assert!(partial > 0);
}

#[test]
fn transform_blur_with_linear_sampling_filters_straight_alpha_premultiplied() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let blur = |source: u64| {
        NodeKind::Transform(Transform {
            id: NodeId::new(3),
            params: TransformParamsDelegate {
                translate_x: expression("0.5 + frame * 2.999"),
                motion_blur: Deferred::value(360.0),
                sampling: TransformSampling::Linear.into(),
                ..Default::default()
            },
            source: PortRef::new(NodeId::new(source), "output".to_string()),
        })
    };
    let convert = |id: u64, source: u64, mode: &str| {
        NodeKind::AlphaPremultiply(AlphaPremultiply {
            id: NodeId::new(id),
            params: AlphaPremultiplyParamsDelegate {
                mode: Deferred::value(mode.to_string()),
            },
            source: PortRef::new(NodeId::new(source), "output".to_string()),
        })
    };
    // Half transparent red, translated by fractions of a pixel so its edge is filtered.
    let premultiplied = render_drawable_frame(
        &mut renderer,
        vec![solid(NodeId::new(1), [255, 0, 0, 128]), blur(1)],
        64,
        0,
        &EmptyMediaStore,
    );
    let straight = render_drawable_frame(
        &mut renderer,
        vec![
            solid(NodeId::new(1), [255, 0, 0, 128]),
            convert(2, 1, "unpremultiply"),
            blur(2),
            convert(4, 3, "premultiply"),
        ],
        64,
        0,
        &EmptyMediaStore,
    );
    let difference = max_channel_difference(&straight, &premultiplied);
    eprintln!("straight vs premultiplied linear blur: max difference {difference}");
    assert!(partially_covered(&premultiplied) > 0);
    assert!(difference <= 2, "max difference {difference}");
}

/// Renders `text` blurred at frame 0, moving 2.999 pixels over the shutter, which takes
/// three taps at one pixel apart, and checks it against the average of the text held at
/// each of those positions.
fn assert_blurred_text_matches_taps(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    text: impl Fn(Deferred<(f64, f64)>, f64) -> NodeKind,
    name: &str,
) {
    let render = |renderer: &mut _, position, motion_blur| {
        render_drawable_frame(
            renderer,
            vec![text(position, motion_blur)],
            128,
            0,
            &EmptyMediaStore,
        )
    };
    let blurred = render(renderer, expression("vec2(20 + frame * 2.999, 30)"), 360.0);
    let still = render(renderer, Deferred::value((20.0, 30.0)), 0.0);
    let taps: Vec<_> = [19.0, 20.0, 21.0]
        .map(|x| render(renderer, Deferred::value((x, 30.0)), 0.0))
        .into();
    let difference = max_channel_difference(&blurred, &average_renders(&taps));
    eprintln!("{name}: max difference {difference} from the taps");
    assert!(max_channel_difference(&blurred, &still) > 16, "{name}");
    assert!(difference <= 4, "{name}: max difference {difference}");
}

#[test]
fn blurred_text_composites_glyphs_and_paint_before_averaging() {
    use lumen_engine::node::{
        source::text::TextRenderMode,
        vector::paint::{GradientPaint, GradientStop, GradientUnits},
    };
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        // Combining marks overlap their base glyphs, and the paint and the layer are
        // translucent, so summing glyphs would count the overlaps twice.
        assert_blurred_text_matches_taps(
            &mut renderer,
            |position, motion_blur| {
                NodeKind::Text(Text {
                    id: NodeId::new(1),
                    params: TextParamsDelegate {
                        content: Deferred::value("A\u{338} f\u{301}".to_string()),
                        font_size: Deferred::value(48.0),
                        render_mode: mode.into(),
                        position,
                        opacity: Deferred::value(0.6),
                        color: Paint::solid([255, 255, 255, 128]).into(),
                        motion_blur: Deferred::value(motion_blur),
                        ..Default::default()
                    },
                })
            },
            &format!("{mode:?} overlapping marks"),
        );
    }
    // A hard gradient stop falls within the enlarged glyph, and paint supersampling
    // antialiases it.
    assert_blurred_text_matches_taps(
        &mut renderer,
        |position, motion_blur| {
            NodeKind::Text(Text {
                id: NodeId::new(1),
                params: TextParamsDelegate {
                    content: Deferred::value("H".to_string()),
                    font_size: Deferred::value(40.0),
                    render_mode: TextRenderMode::Msdf.into(),
                    position,
                    scale: Deferred::value((2.0, 2.0)),
                    paint_supersample: Deferred::value(true),
                    color: Paint::Gradient(GradientPaint {
                        units: GradientUnits::ObjectBoundingBox,
                        start: [0.5, 0.0],
                        end: [0.52, 0.0],
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: [255, 0, 0, 255],
                            },
                            GradientStop {
                                offset: 0.5,
                                color: [255, 0, 0, 255],
                            },
                            GradientStop {
                                offset: 0.5,
                                color: [0, 0, 255, 255],
                            },
                            GradientStop {
                                offset: 1.0,
                                color: [0, 0, 255, 255],
                            },
                        ],
                        ..Default::default()
                    })
                    .into(),
                    motion_blur: Deferred::value(motion_blur),
                    ..Default::default()
                },
            })
        },
        "supersampled gradient",
    );
}

#[test]
fn blurred_text_entering_from_beyond_the_edge_keeps_its_smear() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = |position: Deferred<(f64, f64)>, motion_blur: f64| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value("Lumen".to_string()),
                font_size: Deferred::value(32.0),
                position,
                motion_blur: Deferred::value(motion_blur),
                ..Default::default()
            },
        })]
    };
    // At frame 0 the text lies wholly beyond the left edge, and the shutter carries it 400
    // pixels across, through 32 taps.
    let at = |frame: f64| Deferred::value((-150.0 + frame * 400.0, 40.0));
    let still = render_drawable_frame(&mut renderer, text(at(0.0), 0.0), 128, 0, &EmptyMediaStore);
    assert_eq!(total_alpha(&still), 0);
    let blurred = render_drawable_frame(
        &mut renderer,
        text(expression("vec2(-150 + frame * 400, 40)"), 360.0),
        128,
        0,
        &EmptyMediaStore,
    );
    let taps: Vec<_> = tap_frames(0, 360.0, 32)
        .into_iter()
        .map(|frame| {
            render_drawable_frame(
                &mut renderer,
                text(at(frame), 0.0),
                128,
                0,
                &EmptyMediaStore,
            )
        })
        .collect();
    let (blurred, expected) = (total_alpha(&blurred), total_alpha(&average_renders(&taps)));
    eprintln!("entering text: total alpha {blurred} vs {expected} from the taps");
    assert!(expected > 0 && blurred.abs_diff(expected) * 20 < expected);
}

#[test]
fn blurred_text_scrolling_beyond_the_canvas_is_not_clipped() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = |position: Deferred<(f64, f64)>, motion_blur: f64| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value("MMMMMMMMMMMM".to_string()),
                font_size: Deferred::value(32.0),
                position,
                motion_blur: Deferred::value(motion_blur),
                ..Default::default()
            },
        })]
    };
    // The text is far wider than the canvas, and the shutter slides it 64 pixels, so the
    // taps show a region of the text wider than the canvas.
    let blurred = render_drawable_frame(
        &mut renderer,
        text(expression("vec2(-60 + frame * 64, 40)"), 360.0),
        128,
        0,
        &EmptyMediaStore,
    );
    let taps: Vec<_> = tap_frames(0, 360.0, 32)
        .into_iter()
        .map(|frame| {
            render_drawable_frame(
                &mut renderer,
                text(Deferred::value((-60.0 + frame * 64.0, 40.0)), 0.0),
                128,
                0,
                &EmptyMediaStore,
            )
        })
        .collect();
    let expected = average_renders(&taps);
    let (blurred_alpha, expected_alpha) = (total_alpha(&blurred), total_alpha(&expected));
    eprintln!("scrolling text: total alpha {blurred_alpha} vs {expected_alpha} from the taps");
    assert!(blurred_alpha.abs_diff(expected_alpha) * 50 < expected_alpha);
    let last_column = |pixels: &[u8]| -> u64 {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .skip(127)
            .step_by(128)
            .map(|pixel| u64::from(pixel[3]))
            .sum()
    };
    assert!(last_column(&expected) > 0);
    assert!(last_column(&blurred).abs_diff(last_column(&expected)) * 10 < last_column(&expected));
}

#[test]
fn blurred_msdf_text_keeps_its_resolution_whichever_way_the_scale_changes() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let text = |scale: Deferred<(f64, f64)>, motion_blur: f64| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value("Hi".to_string()),
                font_size: Deferred::value(32.0),
                render_mode: TextRenderMode::Msdf.into(),
                position: Deferred::value((10.0, 10.0)),
                scale,
                motion_blur: Deferred::value(motion_blur),
                ..Default::default()
            },
        })]
    };
    // Frame 1 scales (0.5, 0.5), after (3, 0.5) at the shutter's open, and before
    // (0.5, 3) at its close; the reverse swaps the axes.
    let opening = |renderer: &mut _, x: &str, y: &str| {
        let scale = format!("vec2({x}, {y})");
        render_drawable_frame(
            renderer,
            text(expression(&scale), 360.0),
            192,
            1,
            &EmptyMediaStore,
        )
    };
    let forward = opening(
        &mut renderer,
        "0.5 + 5 * max(0, 1 - frame)",
        "0.5 + 5 * max(0, frame - 1)",
    );
    let reverse = opening(
        &mut renderer,
        "0.5 + 5 * max(0, frame - 1)",
        "0.5 + 5 * max(0, 1 - frame)",
    );
    let reversed_difference = max_channel_difference(&forward, &reverse);
    eprintln!("anisotropic scale: reversed by {reversed_difference}");
    assert!(
        reversed_difference <= 2,
        "reversal changes {reversed_difference}"
    );

    let taps: Vec<_> = tap_frames(1, 360.0, 32)
        .into_iter()
        .map(|frame| {
            let scale = (
                0.5 + 5.0 * (1.0 - frame).max(0.0),
                0.5 + 5.0 * (frame - 1.0).max(0.0),
            );
            render_drawable_frame(
                &mut renderer,
                text(Deferred::value(scale), 0.0),
                192,
                1,
                &EmptyMediaStore,
            )
        })
        .collect();
    let expected = average_renders(&taps);
    let mean_error = forward
        .iter()
        .zip(&expected)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum::<u64>() as f64
        / forward.len() as f64;
    eprintln!("anisotropic scale: mean error {mean_error:.3} from the taps");
    assert!(mean_error < 1.0, "mean error {mean_error}");
}

fn wgsl_shader_composition(
    shader: &str,
    uniforms: serde_json::Value,
    inputs: &[[u8; 4]],
) -> Composition {
    let (node, output) = (NodeId::new(10), NodeId::new(11));
    let mut graph = Graph::new();
    let mut sources = Vec::new();
    for (index, color) in inputs.iter().enumerate() {
        let id = NodeId::new(index as u64 + 1);
        graph.nodes.insert(id, solid(id, *color));
        sources.push(PortRef::new(id, "output".to_string()));
    }
    graph.nodes.insert(
        node,
        NodeKind::WgslShader(WgslShader {
            id: node,
            params: WgslShaderParamsDelegate {
                shader: Deferred::value(shader.to_string()),
                uniforms: Deferred::value(
                    ShaderUniforms::from_json_value(&uniforms, "uniforms").unwrap(),
                ),
            },
            sources,
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(node, "output".to_string()),
        }),
    );
    test_composition(graph)
}

fn assert_uniform_pixels(pixels: &[u8], expected: [u8; 4], message: &str) {
    for pixel in pixels.as_chunks::<4>().0 {
        assert!(
            pixel
                .iter()
                .zip(expected)
                .all(|(channel, expected)| channel.abs_diff(expected) <= 1),
            "{message}: expected {expected:?}, got {pixel:?}"
        );
    }
}

const WGSL_GLOBALS_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = vec4<f32>(lumen.time, lumen.resolution / 255.0, f32(lumen.frame) / 255.0);
    textureStore(output, vec2<i32>(id.xy), color);
}
";

#[test]
fn wgsl_shader_exposes_time_frame_and_resolution_without_uniforms() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = wgsl_shader_composition(WGSL_GLOBALS_SHADER, json!({}), &[[0, 0, 0, 255]]);
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [0, 8, 4, 0], "frame 0 of an 8x4 output");
    let pixels = render_rgba8(&mut renderer, &composition, 6, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [64, 8, 4, 6], "frame 6 at 24 fps is 0.25 s");
}

#[test]
fn wgsl_shader_binds_typed_uniforms_with_vec3_alignment() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shader = "
struct Params {
    amount: f32,
    tint: vec3<f32>,
    alpha: f32,
    count: u32,
}
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = vec4<f32>(params.tint.xy, params.amount * f32(params.count), params.alpha);
    textureStore(output, vec2<i32>(id.xy), color);
}
";
    let uniforms = json!({ "tint": "#ff8000", "amount": 0.125, "count": 2, "alpha": "=time * 2" });
    let composition = wgsl_shader_composition(shader, uniforms, &[[0, 0, 0, 255]]);
    let pixels = render_rgba8(&mut renderer, &composition, 6, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [255, 128, 64, 128], "typed uniforms");
}

#[test]
fn wgsl_shader_binds_three_inputs_with_shared_sampler() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shader = "
struct Params {
    gain: vec3<f32>,
}
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let uv = (vec2<f32>(id.xy) + 0.5) / lumen.resolution;
    let red = textureLoad(input0, vec2<i32>(id.xy), 0).r;
    let green = textureSampleLevel(input1, input_sampler, uv, 0.0).g;
    let blue = textureSampleLevel(input2, input_sampler, uv, 0.0).b;
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(vec3<f32>(red, green, blue) * params.gain, 1.0));
}
";
    let composition = wgsl_shader_composition(
        shader,
        json!({ "gain": [1, 2, 4] }),
        &[[200, 0, 0, 255], [0, 50, 0, 255], [0, 0, 25, 255]],
    );
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [200, 100, 100, 255], "one channel from each input");
}

#[test]
fn wgsl_shader_recompiles_when_source_changes() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shader = |color: &str| {
        format!(
            "@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {{
    textureStore(output, vec2<i32>(id.xy), {color});
}}"
        )
    };
    let red = wgsl_shader_composition(
        &shader("vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
        json!({}),
        &[[0; 4]],
    );
    let pixels = render_rgba8(&mut renderer, &red, 0, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [255, 0, 0, 255], "initial source");
    let green = wgsl_shader_composition(
        &shader("vec4<f32>(0.0, 1.0, 0.0, 1.0)"),
        json!({}),
        &[[0; 4]],
    );
    let pixels = render_rgba8(&mut renderer, &green, 0, &EmptyMediaStore);
    assert_uniform_pixels(
        &pixels,
        [0, 255, 0, 255],
        "edited source must rebuild the pipeline",
    );
}

#[test]
fn wgsl_shader_keeps_v1_shaders_and_rejects_bad_uniforms() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let v1 = "
struct ShaderParams {
    values: vec4<f32>,
}
@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ShaderParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    textureStore(output_tex, vec2<i32>(id.xy), color + params.values);
}
";
    let composition =
        wgsl_shader_composition(v1, json!({ "values": [0.5, 0, 0, 0] }), &[[0, 0, 0, 255]]);
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [128, 0, 0, 255], "v1 shader");

    for (shader, uniforms, expected) in [
        (
            v1,
            json!({ "tint": [1, 0, 0, 0] }),
            "uniforms.tint: `ShaderParams` has no such field",
        ),
        (
            v1,
            json!({ "values": [1, 0, 0] }),
            "uniforms.values: vec4<f32> needs 4 components",
        ),
        (
            "struct P { m: mat4x4<f32> }\n@group(0) @binding(1) var<uniform> p: P;",
            json!({}),
            "unsupported type `mat4x4<f32>`",
        ),
    ] {
        let composition = wgsl_shader_composition(shader, uniforms.clone(), &[[0, 0, 0, 255]]);
        let Some(mut renderer) = gpu_composition_renderer() else {
            return;
        };
        let error = renderer
            .render_frame(&composition, 0, &EmptyMediaStore)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{uniforms}: {error}");
    }
}

fn fill_shader(color: &str) -> String {
    format!(
        "@compute @workgroup_size(8, 8, 1) fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) \
         {{ textureStore(output, vec2<i32>(id.xy), {color}); }}"
    )
}

fn wgsl_shader_node(id: NodeId, shader: Deferred<String>, sources: Vec<PortRef>) -> NodeKind {
    NodeKind::WgslShader(WgslShader {
        id,
        params: WgslShaderParamsDelegate {
            shader,
            uniforms: Deferred::value(ShaderUniforms::default()),
        },
        sources,
    })
}

#[test]
fn wgsl_shader_reports_oversized_uniforms_and_gpu_validation_errors() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let fields: String = (0..4097)
        .map(|index| format!("f{index}: vec4<f32>,"))
        .collect();
    let oversized = format!(
        "struct Big {{ {fields} }}\n@group(0) @binding(1) var<uniform> big: Big;\n{}",
        fill_shader("big.f0")
    );
    let mismatched_binding = "
@group(0) @binding(0) var input_tex: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(input_tex, vec2<i32>(id.xy), vec4<f32>(1.0));
}
"
    .to_string();
    for (shader, expected) in [
        (oversized, "limited to 65536 bytes"),
        (fill_shader("vec4<f32>(1.0"), "GPU validation failed"),
        (mismatched_binding, "GPU validation failed"),
    ] {
        let composition = wgsl_shader_composition(&shader, json!({}), &[[0, 0, 0, 255]]);
        let error = renderer
            .render_frame(&composition, 0, &EmptyMediaStore)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{expected}: {error}");
    }
    let valid = wgsl_shader_composition(&fill_shader("vec4<f32>(1.0)"), json!({}), &[[0; 4]]);
    let pixels = render_rgba8(&mut renderer, &valid, 0, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [255; 4], "the renderer survives failed shaders");
}

#[test]
fn wgsl_shader_source_edits_drop_superseded_plans_but_not_held_textures() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut outputs = Vec::new();
    let mut held = None;
    for edit in 0..12u8 {
        let color = format!("vec4<f32>({edit}.0 / 255.0, 0.0, 0.0, 1.0)");
        let composition = wgsl_shader_composition(&fill_shader(&color), json!({}), &[[0; 4]]);
        let raster = renderer
            .render_frame(&composition, 0, &EmptyMediaStore)
            .unwrap();
        let output = renderer.gpu_renderer().texture_arc(raster.texture).unwrap();
        held.get_or_insert_with(|| (output.as_ref().clone(), raster.domain.storage_size));
        outputs.push(Arc::downgrade(&output));
    }
    let resident = outputs
        .iter()
        .filter(|output| output.strong_count() > 0)
        .count();
    assert_eq!(resident, 1, "superseded plan outputs are still resident");

    let (texture, size) = held.unwrap();
    let pixels = read_texture_rgba8(renderer.gpu_renderer(), &texture, size);
    assert_uniform_pixels(&pixels, [0, 0, 0, 255], "texture held from the first edit");
}

#[test]
fn lookahead_keeps_every_switch_plan_resident() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    const PLANS: u32 = 9;
    let composition = switch_composition(PLANS);

    let mut plan_outputs = std::collections::HashMap::new();
    for frame in 0..4 * PLANS {
        let raster = renderer
            .render_frame(&composition, frame, &EmptyMediaStore)
            .unwrap();
        let output = renderer.gpu_renderer().texture_arc(raster.texture).unwrap();
        let first = plan_outputs
            .entry(frame % PLANS)
            .or_insert_with(|| Arc::clone(&output));
        assert!(
            Arc::ptr_eq(first, &output),
            "frame {frame} recompiled a plan the lookahead had already built"
        );
        renderer
            .precompile_frame_window(&composition, frame + 1, 30, &EmptyMediaStore)
            .unwrap();
        let pixels =
            read_texture_rgba8(renderer.gpu_renderer(), &output, raster.domain.storage_size);
        assert_uniform_pixels(
            &pixels,
            plan_color(frame % PLANS),
            &format!("frame {frame} read after lookahead"),
        );
    }
}

fn switch_composition(plans: u32) -> Composition {
    let (switch, output) = (NodeId::new(100), NodeId::new(101));
    let mut graph = Graph::new();
    let mut layers = Vec::new();
    for index in 0..plans {
        let id = NodeId::new(u64::from(index) + 1);
        graph.nodes.insert(id, solid(id, plan_color(index)));
        layers.push(PortRef::new(id, "output".to_string()));
    }
    graph.nodes.insert(
        switch,
        NodeKind::Switch(Switch {
            id: switch,
            params: SwitchParamsDelegate {
                selected_layer: Deferred::Expr(
                    Expression::parse(&format!("frame % {plans}")).unwrap(),
                ),
            },
            layers,
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(switch, "output".to_string()),
        }),
    );
    let mut composition = test_composition(graph);
    composition.timeline.duration_frames = 120;
    composition
}

#[test]
fn lookahead_keeps_the_bound_plan_when_the_composition_is_edited() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shader = fill_shader("vec4<f32>(1.0, 0.0, 0.0, 1.0)");
    let composition = wgsl_shader_composition(&shader, json!({}), &[[0; 4]]);
    let bound = renderer
        .bind_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();

    let edited = wgsl_shader_composition(&format!("// edited\n{shader}"), json!({}), &[[0; 4]]);
    renderer
        .precompile_frame_window(&edited, 0, 4, &EmptyMediaStore)
        .unwrap();

    let (raster, _) = renderer.submit_bound_frame(&bound).unwrap();
    let pixels = read_rgba8(renderer.gpu_renderer(), raster);
    assert_uniform_pixels(
        &pixels,
        [255, 0, 0, 255],
        "bound frame after lookahead edit",
    );
}

#[test]
fn lookahead_compiles_a_rejected_plan_once() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = switch_composition(9);
    renderer
        .render_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    let plan_bytes = renderer.plan_cache_bytes();
    renderer.set_plan_cache_budget(plan_bytes);

    let compiles = renderer.plan_compile_count();
    for _ in 0..6 {
        renderer
            .precompile_frame_window(&composition, 1, 30, &EmptyMediaStore)
            .unwrap();
    }
    assert_eq!(renderer.cached_plan_count(), 1);
    assert_eq!(renderer.plan_compile_count(), compiles + 1);
}

#[test]
fn plan_cache_budget_bounds_independently_animated_shaders() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = animated_shader_composition(6);

    renderer.compile(&composition).unwrap();
    let plan_bytes = renderer.plan_cache_bytes();
    assert!(plan_bytes > 0);
    // The second budget cannot fit even one lookahead plan beside the current one.
    for (budget, max_plans) in [(3 * plan_bytes, 3), (plan_bytes / 2, 2)] {
        renderer.compile(&composition).unwrap();
        renderer.set_plan_cache_budget(budget);
        for frame in 0..72 {
            let raster = renderer
                .render_frame(&composition, frame, &EmptyMediaStore)
                .unwrap();
            let output = renderer.gpu_renderer().texture_arc(raster.texture).unwrap();
            renderer
                .precompile_frame_window(&composition, frame + 1, 30, &EmptyMediaStore)
                .unwrap();
            let current = renderer.gpu_renderer().texture_arc(raster.texture).unwrap();
            assert!(
                Arc::ptr_eq(&current, &output),
                "budget {budget}, frame {frame}: lookahead replaced the current plan"
            );
            assert!(
                renderer.cached_plan_count() <= max_plans
                    && renderer.plan_cache_bytes() <= budget.max(2 * plan_bytes),
                "budget {budget}, frame {frame}: {} plans hold {} bytes",
                renderer.cached_plan_count(),
                renderer.plan_cache_bytes()
            );
            let pixels =
                read_texture_rgba8(renderer.gpu_renderer(), &output, raster.domain.storage_size);
            assert_uniform_pixels(
                &pixels,
                [(frame % 64) as u8, 0, 0, 255],
                &format!("budget {budget}, frame {frame}"),
            );
        }
    }
}

/// Shader `bit` adds 2^bit to red on alternating runs of 2^bit frames, so `bits` shaders use
/// 2^bits distinct plans.
fn animated_shader_composition(bits: u64) -> Composition {
    let input = NodeId::new(1);
    let mut graph = Graph::new();
    graph.nodes.insert(input, solid(input, [0, 0, 0, 255]));
    let mut source = PortRef::new(input, "output".to_string());
    let add_red = |red: u32| {
        format!(
            "@compute @workgroup_size(8, 8, 1) fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) \
             {{ let color = textureLoad(input0, vec2<i32>(id.xy), 0); \
             textureStore(output, vec2<i32>(id.xy), color + vec4<f32>({red}.0 / 255.0, 0.0, 0.0, 0.0)); }}"
        )
    };
    for bit in 0..bits {
        let id = NodeId::new(bit + 2);
        let toggle = format!(
            "if(floor(frame / {}) % 2 == 0, '{}', '{}')",
            1 << bit,
            add_red(0),
            add_red(1 << bit)
        );
        graph.nodes.insert(
            id,
            wgsl_shader_node(
                id,
                Deferred::Expr(Expression::parse(&toggle).unwrap()),
                vec![source],
            ),
        );
        source = PortRef::new(id, "output".to_string());
    }
    let output = NodeId::new(100);
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput { id: output, source }),
    );
    let mut composition = test_composition(graph);
    composition.timeline.duration_frames = 1 << (bits + 1);
    composition
}

fn plan_color(index: u32) -> [u8; 4] {
    [index as u8 * 25 + 5, 0, 0, 255]
}

#[test]
fn rejected_plan_memory_is_bounded_and_released() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = animated_shader_composition(9);
    renderer
        .render_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    renderer.set_plan_cache_budget(0);
    for frame in 1..512 {
        renderer
            .precompile_frame(&composition, frame, &EmptyMediaStore)
            .unwrap();
    }
    assert!(renderer.rejected_plan_count() > 0);
    assert!(renderer.rejected_plan_count() <= 256);

    renderer.release_resources();
    assert_eq!(renderer.rejected_plan_count(), 0);
}

#[test]
fn rendering_precompiled_shader_edits_prunes_superseded_plans() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    for edit in 0..12u8 {
        let color = format!("vec4<f32>({edit}.0 / 255.0, 0.0, 0.0, 1.0)");
        let composition = wgsl_shader_composition(&fill_shader(&color), json!({}), &[[0; 4]]);
        renderer
            .precompile_frame(&composition, 0, &EmptyMediaStore)
            .unwrap();
        renderer
            .render_frame(&composition, 0, &EmptyMediaStore)
            .unwrap();
    }
    assert_eq!(renderer.cached_plan_count(), 1);
}

#[test]
fn shrinking_the_canvas_forgets_rejected_plan_estimates() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut composition = switch_composition(9);
    composition.render_settings.width = 1024;
    composition.render_settings.height = 1024;
    renderer
        .render_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    let plan_bytes = renderer.plan_cache_bytes();
    renderer.set_plan_cache_budget(plan_bytes + plan_bytes / 4);
    renderer
        .precompile_frame(&composition, 1, &EmptyMediaStore)
        .unwrap();
    assert_eq!(renderer.cached_plan_count(), 1);

    composition.render_settings.width = 8;
    composition.render_settings.height = 4;
    renderer
        .precompile_frame(&composition, 1, &EmptyMediaStore)
        .unwrap();
    assert_eq!(renderer.cached_plan_count(), 2);
}

#[test]
fn resizing_the_canvas_does_not_reuse_the_old_plan() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut composition = switch_composition(9);
    composition.render_settings.width = 8;
    composition.render_settings.height = 4;
    let raster = renderer
        .render_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    assert_eq!(raster.domain.storage_size, lumen_gpu::Size::new(8, 4));

    composition.render_settings.width = 16;
    composition.render_settings.height = 8;
    let raster = renderer
        .render_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    assert_eq!(raster.domain.storage_size, lumen_gpu::Size::new(16, 8));
    assert_eq!(renderer.cached_plan_count(), 1);
}

#[test]
fn plan_key_reports_time_remap_cycles() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (remap, output) = (NodeId::new(1), NodeId::new(2));
    let mut graph = Graph::new();
    graph.nodes.insert(
        remap,
        NodeKind::TimeRemap(TimeRemap {
            id: remap,
            params: TimeRemapParamsDelegate {
                frame: Deferred::value(0.0),
                loop_enabled: Deferred::value(false),
                loop_start: Deferred::value(0),
                loop_end: Deferred::value(0),
            },
            source: PortRef::new(remap, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(remap, "output".to_string()),
        }),
    );
    let error = renderer.compile(&test_composition(graph)).unwrap_err();
    assert!(
        matches!(
            error,
            LumenError::GraphValidation(GraphValidationError::Cycle { .. })
        ),
        "{error}"
    );
}

#[test]
fn wgsl_shader_source_keys_follow_time_remap_branches() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (input, shader, left, right, combine, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
        NodeId::new(5),
        NodeId::new(6),
    );
    let mut graph = Graph::new();
    graph.nodes.insert(input, solid(input, [0, 0, 0, 255]));
    let source = format!(
        "if(frame < 5, '{}', '{}')",
        fill_shader("vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
        fill_shader("vec4<f32>(0.0, 1.0, 0.0, 1.0)")
    );
    graph.nodes.insert(
        shader,
        wgsl_shader_node(
            shader,
            Deferred::Expr(Expression::parse(&source).unwrap()),
            vec![PortRef::new(input, "output".to_string())],
        ),
    );
    for (id, remap) in [
        (left, "if(frame % 2 == 0, 0, 9)"),
        (right, "if(frame % 2 == 0, 9, 0)"),
    ] {
        graph.nodes.insert(
            id,
            NodeKind::TimeRemap(TimeRemap {
                id,
                params: TimeRemapParamsDelegate {
                    frame: Deferred::Expr(Expression::parse(remap).unwrap()),
                    loop_enabled: Deferred::value(false),
                    loop_start: Deferred::value(0),
                    loop_end: Deferred::value(0),
                },
                source: PortRef::new(shader, "output".to_string()),
            }),
        );
    }
    let halves = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    var color = textureLoad(input0, vec2<i32>(id.xy), 0);
    if (id.x >= 4u) {
        color = textureLoad(input1, vec2<i32>(id.xy), 0);
    }
    textureStore(output, vec2<i32>(id.xy), color);
}
";
    graph.nodes.insert(
        combine,
        wgsl_shader_node(
            combine,
            Deferred::value(halves.to_string()),
            vec![
                PortRef::new(left, "output".to_string()),
                PortRef::new(right, "output".to_string()),
            ],
        ),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(combine, "output".to_string()),
        }),
    );
    let composition = test_composition(graph);

    let (red, green) = ([255, 0, 0, 255], [0, 255, 0, 255]);
    for (frame, expected) in [(0, [red, green]), (1, [green, red]), (2, [red, green])] {
        let pixels = render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore);
        for (row, pixels) in pixels.chunks(8 * 4).enumerate() {
            let (left, right) = pixels.split_at(4 * 4);
            assert_uniform_pixels(left, expected[0], &format!("frame {frame} row {row} left"));
            assert_uniform_pixels(
                right,
                expected[1],
                &format!("frame {frame} row {row} right"),
            );
        }
    }
}

#[test]
fn wgsl_shader_source_edits_behind_a_filter_rebuild_the_plan() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let through_opacity = |color: &str| {
        let (input, shader, opacity, output) = (
            NodeId::new(1),
            NodeId::new(2),
            NodeId::new(3),
            NodeId::new(4),
        );
        let mut graph = Graph::new();
        graph.nodes.insert(input, solid(input, [0, 0, 0, 255]));
        graph.nodes.insert(
            shader,
            wgsl_shader_node(
                shader,
                Deferred::value(fill_shader(color)),
                vec![PortRef::new(input, "output".to_string())],
            ),
        );
        graph.nodes.insert(
            opacity,
            NodeKind::Opacity(Opacity {
                id: opacity,
                source: PortRef::new(shader, "output".to_string()),
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
        test_composition(graph)
    };
    let red = through_opacity("vec4<f32>(1.0, 0.0, 0.0, 1.0)");
    let pixels = render_rgba8(&mut renderer, &red, 0, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [255, 0, 0, 255], "initial source");
    let green = through_opacity("vec4<f32>(0.0, 1.0, 0.0, 1.0)");
    let pixels = render_rgba8(&mut renderer, &green, 0, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [0, 255, 0, 255], "edit behind opacity");
}

#[test]
fn wgsl_shader_premultiplies_straight_inputs() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let (opaque, half_red, straight, shader, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
        NodeId::new(5),
    );
    let mut graph = Graph::new();
    graph.nodes.insert(opaque, solid(opaque, [0, 0, 255, 255]));
    graph
        .nodes
        .insert(half_red, solid(half_red, [255, 0, 0, 128]));
    graph.nodes.insert(
        straight,
        NodeKind::AlphaPremultiply(AlphaPremultiply {
            id: straight,
            params: AlphaPremultiplyParamsDelegate {
                mode: Deferred::value("unpremultiply".to_string()),
            },
            source: PortRef::new(half_red, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        shader,
        wgsl_shader_node(
            shader,
            Deferred::value(fill_shader("textureLoad(input1, vec2<i32>(id.xy), 0)")),
            vec![
                PortRef::new(opaque, "output".to_string()),
                PortRef::new(straight, "output".to_string()),
            ],
        ),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(shader, "output".to_string()),
        }),
    );
    let composition = test_composition(graph);
    let raster = renderer
        .render_frame(&composition, 0, &EmptyMediaStore)
        .unwrap();
    assert_eq!(raster.metadata.alpha_mode, AlphaMode::Premultiplied);
    let pixels = read_rgba8(renderer.gpu_renderer(), raster);
    assert_uniform_pixels(&pixels, [128, 0, 0, 128], "straight input1, premultiplied");
}

#[test]
fn wgsl_shader_parses_declarations_with_any_whitespace() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shader = format!(
        "struct  Params{{ tint :vec3 < f32 > ,alpha : f32 }}\n\
         @ group( 0 ) @binding( 1u ) var< uniform > params : Params ;\n{}",
        fill_shader("vec4<f32>(params.tint, params.alpha)")
    );
    let composition = wgsl_shader_composition(
        &shader,
        json!({ "tint": [0.5, 0.25, 1], "alpha": 1 }),
        &[[0, 0, 0, 255]],
    );
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [128, 64, 255, 255], "whitespace-tolerant uniforms");
}

/// The node JSON that `wgslShader(...)` in `@lumiscia/lumen-shared` emits.
#[cfg(feature = "json")]
#[test]
fn wgsl_shader_renders_typed_uniforms_from_json() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let shader = "diagnostic(off, derivative_uniformity);

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let red = uniforms.tint.r * uniforms.strength * f32(uniforms.steps);
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(red, uniforms.center / 255.0, uniforms.tint.a));
}

struct Uniforms {
    strength: f32,
    center: vec2<f32>,
    tint: vec4<f32>,
    steps: u32,
}
@group(0) @binding(1) var<uniform> uniforms: Uniforms;
";
    let composition = lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 24, "duration_frames": 12 },
        "render_settings": { "width": 8, "height": 4 },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255], "width": 8, "height": 4 } },
            { "id": 2, "type": "wgsl_shader", "params": {
                "shader": shader,
                "uniforms": { "strength": 0.25, "center": [64, "=frame * 2"], "tint": "#ff0000", "steps": 2 }
            } },
            { "id": 3, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 2, "to_port": "source" },
            { "from_node": 2, "to_node": 3, "to_port": "source" }
        ]
    }))
    .unwrap();
    let pixels = render_rgba8(&mut renderer, &composition, 6, &EmptyMediaStore);
    assert_uniform_pixels(&pixels, [128, 64, 12, 255], "uniforms from JSON");
}

fn glyph_text(content: &str, params: TextParamsDelegate) -> Vec<NodeKind> {
    vec![NodeKind::Text(Text {
        id: NodeId::new(1),
        params: TextParamsDelegate {
            content: Deferred::value(content.to_string()),
            font_size: Deferred::value(40.0),
            position: Deferred::value((20.0, 10.0)),
            ..params
        },
    })]
}

fn alpha_of(pixels: &[u8], size: u32, x: usize, y: usize) -> u8 {
    pixels[(y * size as usize + x) * 4 + 3]
}

/// Runs of columns that hold any ink, left to right.
fn inked_columns(pixels: &[u8], size: u32) -> Vec<std::ops::Range<usize>> {
    let size = size as usize;
    let inked = |x: usize| (0..size).any(|y| alpha_of(pixels, size as u32, x, y) > 0);
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for x in (0..size).filter(|&x| inked(x)) {
        match runs.last_mut() {
            Some(run) if run.end == x => run.end = x + 1,
            _ => runs.push(x..x + 1),
        }
    }
    runs
}

/// `[min_x, min_y, max_x, max_y]` of the pixels in `columns` with alpha over half.
fn ink_box(pixels: &[u8], size: u32, columns: std::ops::Range<usize>) -> [usize; 4] {
    let mut bounds = [usize::MAX, usize::MAX, 0, 0];
    for y in 0..size as usize {
        for x in columns.clone() {
            if alpha_of(pixels, size, x, y) > 128 {
                bounds = [
                    bounds[0].min(x),
                    bounds[1].min(y),
                    bounds[2].max(x),
                    bounds[3].max(y),
                ];
            }
        }
    }
    bounds
}

#[test]
fn bounded_text_holds_glyphs_its_glyph_params_move() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let text = |position: (f64, f64), glyph_offset: (f64, f64)| {
        vec![NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value("Hi".to_string()),
                font_size: Deferred::value(30.0),
                position: Deferred::value(position),
                glyph_offset: Deferred::value(glyph_offset),
                ..Default::default()
            },
        })]
    };
    let moved = render_drawable_graph(&mut renderer, text((10.0, 10.0), (0.0, 60.0)), size);
    let placed = render_drawable_graph(&mut renderer, text((10.0, 70.0), (0.0, 0.0)), size);
    assert!(placed.iter().skip(3).step_by(4).any(|&a| a > 0));
    assert_eq!(moved, placed);
}

#[test]
fn text_glyph_offset_and_opacity_stagger_each_glyph() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        let params = || TextParamsDelegate {
            render_mode: mode.into(),
            ..Default::default()
        };
        let still = render_drawable_graph(&mut renderer, glyph_text("IIII", params()), size);
        let staggered = render_drawable_graph(
            &mut renderer,
            glyph_text(
                "IIII",
                TextParamsDelegate {
                    glyph_offset: expression("vec2(0, glyph * 10)"),
                    glyph_opacity: expression("glyph / (glyph_count - 1)"),
                    ..params()
                },
            ),
            size,
        );
        let bars = inked_columns(&still, size);
        assert_eq!(bars.len(), 4, "{mode:?}");
        // Each bar moves down 10 pixels more than the last, and fades in from the first.
        for (index, bar) in bars.into_iter().enumerate() {
            let opacity = index as f32 / 3.0;
            for y in 0..size as usize {
                for x in bar.clone() {
                    let expected = y
                        .checked_sub(10 * index)
                        .map_or(0.0, |y| f32::from(alpha_of(&still, size, x, y)) * opacity);
                    let actual = f32::from(alpha_of(&staggered, size, x, y));
                    assert!(
                        (actual - expected).abs() <= 1.5,
                        "{mode:?} glyph {index} at ({x}, {y}): {actual} vs {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn glyph_opacity_fades_a_cluster_of_overlapping_instances_as_one() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        // The overlay is a second instance in the base's cluster, and they overlap.
        let render = |renderer: &mut _, opacity| {
            let params = TextParamsDelegate {
                render_mode: mode.into(),
                glyph_opacity: Deferred::value(opacity),
                ..Default::default()
            };
            render_drawable_graph(renderer, glyph_text("A\u{338}", params), size)
        };
        let opaque = render(&mut renderer, 1.0);
        let faded = render(&mut renderer, 0.5);
        assert!(
            opaque.iter().skip(3).step_by(4).any(|&a| a == 255),
            "{mode:?}"
        );
        for (index, (full, half)) in opaque.iter().zip(&faded).enumerate() {
            assert!(
                (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= 1.5,
                "{mode:?} byte {index}: {full} at half opacity gave {half}"
            );
        }
    }
}

#[test]
fn glyph_opacity_keeps_a_cluster_split_by_wrapping_apart() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        // Wrapping puts the base and its mark on separate lines, each moved by its own offset.
        let render = |renderer: &mut _, opacity| {
            let params = TextParamsDelegate {
                render_mode: mode.into(),
                max_width: Deferred::value(15.0),
                glyph_offset: expression("vec2(line * 40, 0)"),
                glyph_opacity: Deferred::value(opacity),
                ..Default::default()
            };
            render_drawable_graph(renderer, glyph_text("A\u{338}", params), size)
        };
        let opaque = render(&mut renderer, 1.0);
        let faded = render(&mut renderer, 0.5);
        for (index, (full, half)) in opaque.iter().zip(&faded).enumerate() {
            assert!(
                (f32::from(*full) * 0.5 - f32::from(*half)).abs() <= 1.5,
                "{mode:?} byte {index}: {full} at half opacity gave {half}"
            );
        }
    }
}

#[test]
fn text_glyph_rotation_turns_each_word_about_its_glyphs() {
    use lumen_engine::node::source::text::TextRenderMode;
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 160;
    for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
        let params = || TextParamsDelegate {
            render_mode: mode.into(),
            ..Default::default()
        };
        let still = render_drawable_graph(&mut renderer, glyph_text("II   II", params()), size);
        let turned = render_drawable_graph(
            &mut renderer,
            glyph_text(
                "II   II",
                TextParamsDelegate {
                    glyph_rotation: expression("word * 90"),
                    ..params()
                },
            ),
            size,
        );
        let bars = inked_columns(&still, size);
        assert_eq!(bars.len(), 4, "{mode:?}");
        // The first word is untouched.
        for y in 0..size as usize {
            for x in bars[0].start..bars[1].end {
                assert_eq!(
                    alpha_of(&turned, size, x, y),
                    alpha_of(&still, size, x, y),
                    "{mode:?} ({x}, {y})"
                );
            }
        }
        // Each glyph of the second turns a quarter about its own middle, so the upright bars
        // lie down where they stood, beside each other.
        let word = bars[1].end + 4..size as usize;
        let [x0, y0, x1, y1] = ink_box(&still, size, word.clone());
        assert!(y1 - y0 > x1 - x0, "{mode:?} still {:?}", [x0, y0, x1, y1]);
        let [tx0, ty0, tx1, ty1] = ink_box(&turned, size, word);
        assert!(
            tx1 - tx0 > 4 * (ty1 - ty0),
            "{mode:?} turned {:?}",
            [tx0, ty0, tx1, ty1]
        );
        let middle = |low: usize, high: usize| (low + high) as f32 / 2.0;
        assert!((middle(tx0, tx1) - middle(x0, x1)).abs() <= 2.0, "{mode:?}");
    }
}

#[test]
fn text_glyph_params_read_the_line_index() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 192;
    let still = render_drawable_graph(
        &mut renderer,
        glyph_text("I\nI\nI", Default::default()),
        size,
    );
    let shifted = render_drawable_graph(
        &mut renderer,
        glyph_text(
            "I\nI\nI",
            TextParamsDelegate {
                glyph_offset: expression("vec2(line * 30, 0)"),
                glyph_opacity: expression("line < line_count - 1"),
                ..Default::default()
            },
        ),
        size,
    );
    // Lines are 48 pixels apart from y = 10. Each moves 30 pixels further right than the
    // last, and the last is hidden.
    for line in 0..3 {
        for y in 10 + 48 * line..10 + 48 * (line + 1) {
            for x in 0..size as usize - 60 {
                let expected = if line < 2 {
                    alpha_of(&still, size, x, y)
                } else {
                    0
                };
                let actual = alpha_of(&shifted, size, x + 30 * line, y);
                assert!(actual.abs_diff(expected) <= 1, "line {line} ({x}, {y})");
            }
        }
    }
}

#[test]
fn constant_glyph_params_match_the_equivalent_static_text() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 128;
    let content = "Lumen fi";
    let mut moved = glyph_text(content, Default::default());
    let NodeKind::Text(text) = &mut moved[0] else {
        unreachable!()
    };
    text.params.position = Deferred::value((32.0, 17.0));
    let moved = render_drawable_graph(&mut renderer, moved, size);
    let offset = render_drawable_graph(
        &mut renderer,
        glyph_text(
            content,
            TextParamsDelegate {
                glyph_offset: Deferred::value((12.0, 7.0)),
                ..Default::default()
            },
        ),
        size,
    );
    assert!(total_alpha(&moved) > 0);
    assert!(max_channel_difference(&moved, &offset) <= 1);
    // An expression that reads no index is the constant it evaluates to.
    let uniform = render_drawable_graph(
        &mut renderer,
        glyph_text(
            content,
            TextParamsDelegate {
                glyph_offset: expression("vec2(12, 7 + 0 * frame)"),
                glyph_scale: expression("vec2(1, 1)"),
                ..Default::default()
            },
        ),
        size,
    );
    assert_eq!(uniform, offset);
}

#[test]
fn glyph_globals_outside_glyph_params_are_an_error() {
    let text = |params: TextParamsDelegate| {
        NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value("Lumen".to_string()),
                ..params
            },
        })
    };
    let shape = NodeKind::Shape(Shape {
        id: NodeId::new(1),
        params: ShapeParamsDelegate {
            opacity: expression("glyph / glyph_count"),
            ..Default::default()
        },
    });
    for (node, name) in [
        (
            text(TextParamsDelegate {
                position: expression("vec2(word * 10, 0)"),
                ..Default::default()
            }),
            "word",
        ),
        (
            text(TextParamsDelegate {
                opacity: expression("line"),
                ..Default::default()
            }),
            "line",
        ),
        (shape, "glyph"),
    ] {
        let mut graph = Graph::new();
        graph.nodes.insert(NodeId::new(1), node);
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
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!(
                "`{name}` is only available in a text node's glyph_offset"
            )),
            "{error}"
        );
    }
}

#[test]
fn glyph_scope_ends_at_property_references_but_crosses_functions() {
    let node = |id: u64, params: TextParamsDelegate| {
        NodeKind::Text(Text {
            id: NodeId::new(id),
            params: TextParamsDelegate {
                content: Deferred::value("Lumen".to_string()),
                ..params
            },
        })
    };
    let bind = |first: TextParamsDelegate| {
        let mut graph = Graph::new();
        graph.nodes.insert(NodeId::new(1), node(1, first));
        graph.nodes.insert(
            NodeId::new(2),
            node(
                2,
                TextParamsDelegate {
                    font_size: expression("20 + glyph * 10"),
                    ..Default::default()
                },
            ),
        );
        graph.nodes.insert(
            NodeId::new(3),
            NodeKind::MediaOutput(MediaOutput {
                id: NodeId::new(3),
                source: PortRef::new(NodeId::new(1), "output".to_string()),
            }),
        );
        let composition = test_composition(graph);
        let compiled = CompileContext::new(&composition).compile().unwrap();
        FrameBindContext::new(&composition, 0)
            .bind(&compiled)
            .map(|_| ())
            .map_err(|error| error.to_string())
    };
    // Another node's property is evaluated outside the glyph, so it fails as it would alone.
    let error = bind(TextParamsDelegate {
        glyph_offset: expression("vec2(node(2, \"font_size\"), 0)"),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.contains("`glyph` is only available"), "{error}");
    let error = bind(TextParamsDelegate {
        glyph_offset: expression("vec2(text_width(node(2)), 0)"),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.contains("`glyph` is only available"), "{error}");

    let definitions = std::collections::BTreeMap::from([(
        "slide(distance)".into(),
        "vec2(distance * glyph, 0)".into(),
    )]);
    let _functions = lumen_engine::expr::FunctionScope::enter(&definitions).unwrap();
    let offset = expression("slide(5)");
    bind(TextParamsDelegate {
        glyph_offset: offset,
        ..Default::default()
    })
    .unwrap();
}

#[test]
fn blurred_text_draws_glyphs_moved_beyond_its_box() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let size = 256;
    let text = |position: Deferred<(f64, f64)>, motion_blur: f64| {
        glyph_text(
            "II",
            TextParamsDelegate {
                position,
                motion_blur: Deferred::value(motion_blur),
                glyph_offset: expression("vec2(0, glyph * 180)"),
                glyph_rotation: expression("glyph * 30"),
                ..Default::default()
            },
        )
    };
    let below = |pixels: &[u8]| total_alpha(&pixels[200 * size as usize * 4..]);
    let blurred = render_drawable_frame(
        &mut renderer,
        text(expression("vec2(10 + frame * 60, 10)"), 180.0),
        size,
        1,
        &EmptyMediaStore,
    );
    let still = render_drawable_frame(
        &mut renderer,
        text(Deferred::value((70.0, 10.0)), 0.0),
        size,
        1,
        &EmptyMediaStore,
    );
    // The second glyph lies wholly below the text's box. It is smeared by the text's motion,
    // and keeps its coverage.
    let (blurred, still) = (below(&blurred), below(&still));
    eprintln!("moved glyph: total alpha {blurred} blurred vs {still} still");
    assert!(still > 0 && blurred.abs_diff(still) * 20 < still);
}

/// A small shape through a `blur`, or a `shadow` when `offset` is given. Number params may be
/// expressions of `frame`.
#[derive(Clone, Copy)]
struct BoundedEffect {
    position: [&'static str; 2],
    offset: Option<[&'static str; 2]>,
    radius: &'static str,
    straight_alpha: bool,
    duration_frames: u32,
}

/// `spec` as a literal when it is a number and `bounded`. Otherwise an expression that reads
/// another node, which no bounds can be worked out for.
fn effect_param(spec: &str, bounded: bool) -> Deferred<f64> {
    match spec.parse::<f64>() {
        Ok(value) if bounded => Deferred::value(value),
        _ if bounded => Deferred::Expr(Expression::parse(spec).unwrap()),
        _ => {
            Deferred::Expr(Expression::parse(&format!("{spec} + 0 * node(99, \"width\")")).unwrap())
        }
    }
}

/// Renders `frames` of `case`, with the pixels and whether the effect's output was bounded.
/// Without `bounded` the shape and the effect fill the canvas, as they would without bounds.
fn render_bounded_effect(
    case: BoundedEffect,
    bounded: bool,
    frames: &[u32],
) -> Option<(Vec<Vec<u8>>, bool)> {
    let (shape, convert, effect, output) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
    );
    let [x, y] = case.position;
    let position = match (x.parse::<f64>(), y.parse::<f64>(), bounded) {
        (Ok(x), Ok(y), true) => Deferred::value((x, y)),
        (.., true) => Deferred::Expr(Expression::parse(&format!("vec2({x}, {y})")).unwrap()),
        _ => Deferred::Expr(
            Expression::parse(&format!("vec2({x} + 0 * node(99, \"width\"), {y})")).unwrap(),
        ),
    };
    let mut graph = Graph::new();
    graph.nodes.insert(
        shape,
        NodeKind::Shape(Shape {
            id: shape,
            params: ShapeParamsDelegate {
                position,
                width: Deferred::value(14),
                height: Deferred::value(11),
                fill_paint: Paint::solid([255, 100, 0, 200]).into(),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        NodeId::new(99),
        NodeKind::Shape(Shape {
            id: NodeId::new(99),
            params: ShapeParamsDelegate::default(),
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
    let radius = effect_param(case.radius, bounded);
    graph.nodes.insert(
        effect,
        match case.offset {
            Some([offset_x, offset_y]) => NodeKind::Shadow(Shadow {
                id: effect,
                params: ShadowParamsDelegate {
                    offset_x: effect_param(offset_x, bounded),
                    offset_y: effect_param(offset_y, bounded),
                    radius,
                    color: Paint::solid([20, 40, 200, 255]).into(),
                    opacity: Deferred::value(0.8),
                },
                source,
            }),
            None => NodeKind::Blur(Blur {
                id: effect,
                params: BlurParamsDelegate { radius },
                source,
            }),
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
    composition.render_settings.width = 384;
    composition.render_settings.height = 288;
    composition.timeline.duration_frames = case.duration_frames;
    let mut renderer = gpu_composition_renderer()?;
    let pixels = frames
        .iter()
        .map(|&frame| render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore))
        .collect();
    let domain = renderer.compiled().unwrap().node_outputs
        [&PortRef::new(effect, "output".to_string())]
        .clone()
        .into_raster(effect, "output")
        .unwrap()
        .domain;
    Some((pixels, !domain.is_full_frame()))
}

/// Asserts that a bounded render matches its full-frame reference up to the rounding of the
/// hardware filter, which depends on the size of the texture read: no channel differs by more
/// than 1, and few bytes differ at all. Coverage the bounded render lost, rather than rounded,
/// shows up as a zero alpha where the reference has some.
fn assert_matches_within_rounding(pixels: &[u8], expected: &[u8], name: &str) {
    assert_eq!(pixels.len(), expected.len(), "{name}: size differs");
    let mut differing = 0usize;
    for (index, (&actual, &expected)) in pixels.iter().zip(expected).enumerate() {
        let difference = actual.abs_diff(expected);
        assert!(
            difference <= 1,
            "{name}: byte {index} is {actual}, expected {expected}"
        );
        differing += usize::from(difference > 0);
    }
    assert!(
        differing * 1000 <= pixels.len(),
        "{name}: {differing} of {} bytes differ",
        pixels.len()
    );
    for (index, (pixel, reference)) in pixels.chunks(4).zip(expected.chunks(4)).enumerate() {
        assert!(
            reference[3] <= 8 || pixel[3] > 0,
            "{name}: pixel {index} lost its coverage, expected alpha {}",
            reference[3]
        );
    }
}

/// Asserts that `case` renders the same bounded as at full frame, and that it is bounded when
/// `expect_bounded`. A blur that reaches past the canvas may still be bounded on one side.
fn assert_bounded_effect_matches_full_frame(
    case: BoundedEffect,
    frames: &[u32],
    expect_bounded: bool,
    name: &str,
) {
    let Some((pixels, bounded)) = render_bounded_effect(case, true, frames) else {
        return;
    };
    let (expected, full) = render_bounded_effect(case, false, frames).unwrap();
    assert!(bounded || !expect_bounded, "{name}: should be bounded");
    assert!(!full, "{name}: the reference should fill the canvas");
    for (frame, (pixels, expected)) in frames.iter().zip(pixels.iter().zip(&expected)) {
        assert!(expected.iter().any(|&byte| byte > 0), "{name}: blank frame");
        assert_matches_within_rounding(pixels, expected, &format!("{name}: frame {frame}"));
    }
}

#[test]
fn bounded_blur_matches_full_frame_near_canvas_edges() {
    // Radii that blur at full resolution, on a reduced level and across two, with the shape in
    // each corner and straight alpha, so that the blur reads its edge and the canvas'. The
    // largest reaches past the canvas on every side.
    for radius in ["0", "3", "8.5", "21", "40", "150"] {
        for position in [["2", "3"], ["368", "274"], ["190", "140"], ["-5", "270"]] {
            for straight_alpha in [false, true] {
                assert_bounded_effect_matches_full_frame(
                    BoundedEffect {
                        position,
                        offset: None,
                        radius,
                        straight_alpha,
                        duration_frames: 1,
                    },
                    &[0],
                    radius != "150",
                    &format!("blur {radius} at {position:?}, straight {straight_alpha}"),
                );
            }
        }
    }
}

#[test]
fn bounded_blur_with_animated_radius_covers_the_largest_radius() {
    // The radius passes through the band where the full level hands over to the reduced one.
    assert_bounded_effect_matches_full_frame(
        BoundedEffect {
            position: ["100", "70"],
            offset: None,
            radius: "2 + frame * 9",
            straight_alpha: false,
            duration_frames: 8,
        },
        &[0, 1, 2, 3, 5, 7],
        true,
        "animated blur radius",
    );
}

#[test]
fn bounded_blur_follows_a_moving_shape() {
    assert_bounded_effect_matches_full_frame(
        BoundedEffect {
            position: ["10 + frame * 30", "40 + frame * 10"],
            offset: None,
            radius: "10",
            straight_alpha: false,
            duration_frames: 8,
        },
        &[0, 2, 4, 7],
        true,
        "blur of a moving shape",
    );
}

#[test]
fn bounded_shadow_covers_its_offset_and_the_source() {
    // Offsets that move the shadow off each edge, and one that moves it off the canvas.
    for (offset, radius) in [
        (["12", "-9"], "6"),
        (["-30", "40"], "25"),
        (["0.4", "7.6"], "0"),
        (["-250", "0"], "12"),
        (["400", "-300"], "60"),
    ] {
        for (position, straight_alpha) in [(["30", "20"], false), (["366", "270"], true)] {
            assert_bounded_effect_matches_full_frame(
                BoundedEffect {
                    position,
                    offset: Some(offset),
                    radius,
                    straight_alpha,
                    duration_frames: 1,
                },
                &[0],
                true,
                &format!("shadow offset {offset:?} radius {radius} at {position:?}"),
            );
        }
    }
    assert_bounded_effect_matches_full_frame(
        BoundedEffect {
            position: ["40", "40"],
            offset: Some(["frame * 12", "-frame * 8"]),
            radius: "4 + frame * 3",
            straight_alpha: false,
            duration_frames: 6,
        },
        &[0, 1, 3, 5],
        true,
        "animated shadow",
    );
}

#[test]
fn blur_and_shadow_fill_the_canvas_for_values_that_cannot_be_bounded() {
    for (offset, radius) in [
        (None, "1000000"),
        (None, "5000000000"),
        (Some(["10000000", "0"]), "4"),
        (Some(["0", "-1000000000000"]), "4"),
    ] {
        let case = BoundedEffect {
            position: ["100", "70"],
            offset,
            radius,
            straight_alpha: false,
            duration_frames: 1,
        };
        let Some((pixels, bounded)) = render_bounded_effect(case, true, &[0]) else {
            return;
        };
        let (expected, _) = render_bounded_effect(case, false, &[0]).unwrap();
        assert!(!bounded, "{offset:?} {radius}: should fill the canvas");
        assert!(pixels == expected, "{offset:?} {radius}: differs");
    }
}

/// Inter, the only family [`render_blurred_text`] resolves.
#[derive(Debug, Clone)]
struct BlurFont;

impl lumen_engine::media::FontResolver for BlurFont {
    fn id(&self) -> &str {
        "bounded-blur-inter"
    }
    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        Ok(vec![
            include_bytes!("../../text/assets/inter/InterVariable-Latin.ttf").to_vec(),
        ])
    }
}

impl MediaStore for BlurFont {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }
    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
    fn font_resolver_id(&self, family: &str) -> Option<String> {
        (family == "Inter").then(|| "bounded-blur-inter".to_string())
    }
    fn get_font_resolver(
        &self,
        family: &str,
    ) -> Option<Box<dyn lumen_engine::media::FontResolver>> {
        (family == "Inter")
            .then(|| Box::new(self.clone()) as Box<dyn lumen_engine::media::FontResolver>)
    }
}

#[derive(Clone, Copy)]
struct BlurredText {
    canvas: (u32, u32),
    position: (f64, f64),
    font_size: f64,
    radius: f64,
    /// Offset of a shadow with the repro's color and opacity, or a plain blur without.
    shadow_offset: Option<(f64, f64)>,
}

/// Renders `frames` of `case`, with the pixels and whether the effect's output was bounded.
/// Without `bounded` the text reads another node, so it and the effect fill the canvas.
fn render_blurred_text(
    case: BlurredText,
    bounded: bool,
    frames: &[u32],
) -> Option<(Vec<Vec<u8>>, bool)> {
    let (text, effect, output) = (NodeId::new(2), NodeId::new(4), NodeId::new(9));
    let (x, y) = case.position;
    let position = if bounded {
        Deferred::value((x, y))
    } else {
        Deferred::Expr(
            Expression::parse(&format!("vec2({x} + 0 * node(99, \"width\"), {y})")).unwrap(),
        )
    };
    let mut graph = Graph::new();
    graph.nodes.insert(
        NodeId::new(99),
        NodeKind::Shape(Shape {
            id: NodeId::new(99),
            params: ShapeParamsDelegate::default(),
        }),
    );
    graph.nodes.insert(
        text,
        NodeKind::Text(Text {
            id: text,
            params: TextParamsDelegate {
                content: Deferred::value("Blur edge".to_string()),
                font_family: Deferred::value("Inter".to_string()),
                font_size: Deferred::value(case.font_size),
                position,
                ..Default::default()
            },
        }),
    );
    let source = PortRef::new(text, "output".to_string());
    let radius = effect_param(&case.radius.to_string(), bounded);
    graph.nodes.insert(
        effect,
        match case.shadow_offset {
            Some((offset_x, offset_y)) => NodeKind::Shadow(Shadow {
                id: effect,
                params: ShadowParamsDelegate {
                    offset_x: Deferred::value(offset_x),
                    offset_y: Deferred::value(offset_y),
                    radius,
                    color: Paint::solid([73, 137, 239, 213]).into(),
                    opacity: Deferred::value(0.83),
                },
                source,
            }),
            None => NodeKind::Blur(Blur {
                id: effect,
                params: BlurParamsDelegate { radius },
                source,
            }),
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
    (
        composition.render_settings.width,
        composition.render_settings.height,
    ) = case.canvas;
    let mut renderer = gpu_composition_renderer()?;
    let pixels = frames
        .iter()
        .map(|&frame| render_rgba8(&mut renderer, &composition, frame, &BlurFont))
        .collect();
    let domain = renderer.compiled().unwrap().node_outputs
        [&PortRef::new(effect, "output".to_string())]
        .clone()
        .into_raster(effect, "output")
        .unwrap()
        .domain;
    Some((pixels, !domain.is_full_frame()))
}

fn assert_blurred_text_matches_full_frame(case: BlurredText, name: &str) {
    let frames = [0, 1, 5];
    let Some((pixels, bounded)) = render_blurred_text(case, true, &frames) else {
        return;
    };
    let (expected, full) = render_blurred_text(case, false, &frames).unwrap();
    assert!(bounded, "{name}: should be bounded");
    assert!(!full, "{name}: the reference should fill the canvas");
    for (frame, (pixels, expected)) in frames.iter().zip(pixels.iter().zip(&expected)) {
        assert!(expected.iter().any(|&byte| byte > 0), "{name}: blank frame");
        assert_matches_within_rounding(pixels, expected, &format!("{name}: frame {frame}"));
    }
}

#[test]
fn bounded_shadow_of_text_matches_full_frame_on_odd_canvases() {
    for (canvas, position, radius, shadow_offset) in [
        ((1027, 773), (338.91, 309.2), 34.15, (-67.5, 32.5)),
        ((1029, 775), (401.37, 288.6), 22.4, (-31.5, 18.5)),
    ] {
        assert_blurred_text_matches_full_frame(
            BlurredText {
                canvas,
                position,
                font_size: 37.0,
                radius,
                shadow_offset: Some(shadow_offset),
            },
            &format!("shadow of text on {canvas:?}"),
        );
    }
}

#[test]
fn bounded_blur_of_text_matches_full_frame_at_fractional_positions() {
    for (canvas, position, font_size, radius, shadow_offset) in [
        ((383, 289), (171.3, 133.6), 29.0, 8.5, None),
        ((385, 291), (-3.4, 277.5), 41.0, 21.0, None),
        ((769, 577), (311.2, 201.1), 33.0, 40.0, None),
        ((769, 577), (311.2, 201.1), 33.0, 12.7, Some((-17.5, 26.4))),
        ((385, 291), (301.7, 22.3), 41.0, 30.6, Some((24.5, -9.5))),
        (
            (1031, 771),
            (355.7, 300.1),
            37.0,
            34.15,
            Some((-67.5, 32.5)),
        ),
        (
            (1111, 777),
            (800.77, 252.78),
            37.0,
            18.2,
            Some((31.5, 32.5)),
        ),
    ] {
        assert_blurred_text_matches_full_frame(
            BlurredText {
                canvas,
                position,
                font_size,
                radius,
                shadow_offset,
            },
            &format!(
                "text at {position:?} on {canvas:?}, radius {radius}, shadow {shadow_offset:?}"
            ),
        );
    }
}

/// A composition whose `probe` template draws its whole canvas with red at 10 × its frame and
/// green at its `g` prop. The root plays `clip` into its output; both use node ids 1 and 2.
fn probe_composition(template_fps: f64, clip: serde_json::Value) -> serde_json::Value {
    json!({
        "timeline": { "fps": 10, "duration_frames": 40 },
        "render_settings": { "width": 1, "height": 1 },
        "compositions": {
            "probe": {
                "render_settings": { "width": 1, "height": 1 },
                "timeline": { "fps": template_fps, "duration_frames": 10 },
                "props": { "g": { "type": "number", "default": 7 } },
                "nodes": [
                    { "id": 1, "type": "background", "params": { "paint": "=rgb(frame * 10, prop('g'), 0)" } },
                    { "id": 2, "type": "media_output" }
                ],
                "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
            }
        },
        "nodes": [
            { "id": 1, "type": "clip", "params": clip },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    })
}

/// The pixel at (0, 0) of each frame, from one renderer.
fn first_pixels(composition: &serde_json::Value, frames: &[u32]) -> Option<Vec<[u8; 4]>> {
    let composition = lumen_engine::json::parse_value(composition).unwrap();
    let mut renderer = gpu_composition_renderer()?;
    Some(
        frames
            .iter()
            .map(|frame| {
                let pixels = render_rgba8(&mut renderer, &composition, *frame, &EmptyMediaStore);
                pixels[..4].try_into().unwrap()
            })
            .collect(),
    )
}

const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn probe(red: u8) -> [u8; 4] {
    [red, 7, 0, 255]
}

#[test]
fn clip_plays_its_template_in_local_time_and_hides_outside_its_window() {
    let composition = probe_composition(
        10.0,
        json!({ "composition": "probe", "start": 2, "duration": 3 }),
    );
    let Some(pixels) = first_pixels(&composition, &[1, 2, 4, 5]) else {
        return;
    };
    assert_eq!(pixels, [CLEAR, probe(0), probe(20), CLEAR]);
}

#[test]
fn clip_end_modes_hold_the_last_frame_or_loop() {
    let hold = probe_composition(
        10.0,
        json!({ "composition": "probe", "start": 2, "duration": 3, "end": "hold" }),
    );
    let loops = probe_composition(
        10.0,
        json!({ "composition": "probe", "start": 2, "duration": 3, "end": "loop" }),
    );
    let (Some(hold), Some(loops)) = (
        first_pixels(&hold, &[1, 4, 5, 30]),
        first_pixels(&loops, &[1, 4, 5, 9]),
    ) else {
        return;
    };
    assert_eq!(hold, [CLEAR, probe(20), probe(20), probe(20)]);
    assert_eq!(loops, [CLEAR, probe(20), probe(0), probe(10)]);
}

#[test]
fn clip_speed_offset_and_template_fps_set_its_local_frame() {
    // local = (frame - start) * speed * template_fps / fps + offset
    let fast = probe_composition(
        10.0,
        json!({ "composition": "probe", "start": 2, "speed": 1.5, "offset": 2 }),
    );
    // The template runs at twice the root's fps, so its 10 frames last 5 root frames.
    let doubled = probe_composition(20.0, json!({ "composition": "probe", "start": 0 }));
    let (Some(fast), Some(doubled)) = (
        first_pixels(&fast, &[2, 3, 4, 7]),
        first_pixels(&doubled, &[3, 4, 5]),
    ) else {
        return;
    };
    // The default duration plays the template's remaining 8 frames: 16 / 3 root frames.
    assert_eq!(fast, [probe(20), probe(35), probe(50), probe(95)]);
    assert_eq!(doubled, [probe(60), probe(80), CLEAR]);
}

#[test]
fn clip_props_take_literals_and_expressions_in_the_parent_time() {
    let literal = probe_composition(
        10.0,
        json!({ "composition": "probe", "start": 2, "props": { "g": 40 } }),
    );
    // Evaluated at the root frame, so it differs from the template's own frame.
    let parent_time = probe_composition(
        10.0,
        json!({ "composition": "probe", "start": 2, "props": { "g": "=frame * 10" } }),
    );
    let (Some(literal), Some(parent_time)) = (
        first_pixels(&literal, &[3]),
        first_pixels(&parent_time, &[3, 5]),
    ) else {
        return;
    };
    assert_eq!(literal, [[10, 40, 0, 255]]);
    assert_eq!(parent_time, [[10, 30, 0, 255], [30, 50, 0, 255]]);
}

#[test]
fn template_used_n_times_renders_each_instance_with_its_props_and_time() {
    let composition = json!({
        "timeline": { "fps": 10, "duration_frames": 40 },
        "render_settings": { "width": 3, "height": 1 },
        "compositions": {
            "probe": {
                "render_settings": { "width": 3, "height": 1 },
                "timeline": { "duration_frames": 10 },
                "props": { "g": { "type": "number" }, "x": { "type": "number" } },
                "nodes": [
                    { "id": 1, "type": "background", "params": { "paint": "=rgb(frame * 10, prop('g'), 0)" } },
                    { "id": 2, "type": "transform", "params": { "translate_x": "=prop('x')" } },
                    { "id": 3, "type": "media_output" }
                ],
                "connections": [
                    { "from_node": 1, "to_node": 2, "to_port": "source" },
                    { "from_node": 2, "to_node": 3, "to_port": "source" }
                ]
            },
            "strip": {
                "render_settings": { "width": 3, "height": 1 },
                "props": { "g": { "type": "number", "default": 50 } },
                "nodes": [
                    { "id": 1, "type": "clip", "params": { "composition": "probe", "start": 0, "props": { "g": "=prop('g')" } } },
                    { "id": 2, "type": "clip", "params": { "composition": "probe", "start": 1, "props": { "g": "=prop('g') + 1", "x": 1 } } },
                    { "id": 3, "type": "merge" },
                    { "id": 4, "type": "media_output" }
                ],
                "connections": [
                    { "from_node": 1, "to_node": 3, "to_port": "base" },
                    { "from_node": 2, "to_node": 3, "to_port": "overlay" },
                    { "from_node": 3, "to_node": 4, "to_port": "source" }
                ]
            }
        },
        "nodes": [
            { "id": 1, "type": "clip", "params": { "composition": "strip", "start": 2 } },
            { "id": 2, "type": "clip", "params": { "composition": "probe", "start": 4, "props": { "g": 99, "x": 2 } } },
            { "id": 3, "type": "merge" },
            { "id": 4, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 3, "to_port": "base" },
            { "from_node": 2, "to_node": 3, "to_port": "overlay" },
            { "from_node": 3, "to_node": 4, "to_port": "source" }
        ]
    });
    let composition = lumen_engine::json::parse_value(&composition).unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let pixels = |renderer: &mut TestRenderer, frame| {
        render_rgba8(renderer, &composition, frame, &EmptyMediaStore)
            .chunks(4)
            .map(|pixel| <[u8; 4]>::try_from(pixel).unwrap())
            .collect::<Vec<_>>()
    };
    // Frame 2: the strip's first probe starts; its second and the root's probe wait.
    assert_eq!(pixels(&mut renderer, 2), [[0, 50, 0, 255]; 3]);
    // Frame 5: the strip is 3 frames in, its second probe 2 and moved 1 px, and the root's
    // probe 1 and moved 2 px; each covers the ones before it.
    assert_eq!(
        pixels(&mut renderer, 5),
        [[30, 50, 0, 255], [20, 51, 0, 255], [10, 99, 0, 255]]
    );
}

#[test]
fn hidden_clip_compiles_and_binds_nothing_upstream() {
    let mut composition = probe_composition(10.0, json!({ "composition": "probe", "start": 5 }));
    composition["nodes"][0]["id"] = json!(10);
    composition["nodes"][1] = json!({ "id": 11, "type": "media_output" });
    composition["connections"][0] = json!({ "from_node": 10, "to_node": 11, "to_port": "source" });
    let composition = lumen_engine::json::parse_value(&composition).unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut stats = |frame| {
        let bound = renderer
            .bind_frame(&composition, frame, &EmptyMediaStore)
            .unwrap();
        let compiled = renderer.compiled().unwrap();
        let background = compiled
            .compiled_nodes
            .get(&NodeId::new(1))
            .map(|instances| instances.keys().cloned().collect::<Vec<_>>());
        (
            compiled.plan.passes().len(),
            bound.buffer_upload_count(),
            background,
        )
    };
    let (hidden_passes, hidden_uploads, hidden_background) = stats(2);
    let (shown_passes, shown_uploads, shown_background) = stats(6);
    assert_eq!(hidden_background, None);
    assert_eq!(shown_background, Some(vec![vec![NodeId::new(10)]]));
    assert!(
        hidden_passes < shown_passes,
        "{hidden_passes} vs {shown_passes}"
    );
    assert!(
        hidden_uploads < shown_uploads,
        "{hidden_uploads} vs {shown_uploads}"
    );
    assert_eq!(renderer.cached_plan_count(), 2);
}

#[test]
fn clip_windows_its_source_like_a_time_remap() {
    let composition = json!({
        "timeline": { "fps": 10, "duration_frames": 20 },
        "render_settings": { "width": 1, "height": 1 },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paint": "=rgb(frame * 10, 0, 0)" } },
            { "id": 2, "type": "clip", "params": { "start": 5, "duration": 4, "offset": 2 } },
            { "id": 3, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 2, "to_port": "source" },
            { "from_node": 2, "to_node": 3, "to_port": "source" }
        ]
    });
    let Some(pixels) = first_pixels(&composition, &[4, 5, 8, 9]) else {
        return;
    };
    assert_eq!(pixels, [CLEAR, [20, 0, 0, 255], [50, 0, 0, 255], CLEAR]);
}

#[test]
fn template_larger_than_its_parent_is_cropped_to_the_parent_canvas() {
    let mut composition = probe_composition(10.0, json!({ "composition": "probe", "start": 0 }));
    composition["render_settings"] = json!({ "width": 3, "height": 2 });
    composition["compositions"]["probe"]["render_settings"] = json!({ "width": 5, "height": 1 });
    let composition = lumen_engine::json::parse_value(&composition).unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let pixels = render_rgba8(&mut renderer, &composition, 2, &EmptyMediaStore);
    assert_eq!(pixels.len(), 3 * 2 * 4);
    assert_eq!(&pixels[..12], [probe(20), probe(20), probe(20)].concat());
    assert_eq!(&pixels[12..], [CLEAR; 3].concat());
}

#[test]
fn animated_sources_in_a_clip_are_bounded_over_the_clip_frames_without_clipping() {
    // The square moves with the template's frame, which the offset starts at 4, so bounds taken
    // over the template's first frames would clip it.
    let composition = json!({
        "timeline": { "fps": 10, "duration_frames": 20 },
        "render_settings": { "width": 16, "height": 4 },
        "compositions": {
            "mover": {
                "timeline": { "duration_frames": 12 },
                "nodes": [
                    { "id": 1, "type": "shape", "params": {
                        "width": 2, "height": 2, "position": "=vec2(frame, 1)",
                        "fill_paint": "#ffffff", "edge_antialias": false
                    } },
                    { "id": 2, "type": "media_output" }
                ],
                "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
            }
        },
        "nodes": [
            { "id": 1, "type": "clip", "params": { "composition": "mover", "start": 10, "offset": 4 } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    });
    let composition = lumen_engine::json::parse_value(&composition).unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    for (frame, x) in [(10, 4), (14, 8), (17, 11)] {
        let pixels = render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore);
        let shape = render_shape_like(x);
        assert_eq!(pixels, shape, "frame {frame}");
    }
    // The square's texture covers x 4..13 of the template's frames 4 to 11, padded by 2 pixels,
    // rather than its whole timeline.
    let bounded = renderer
        .compiled()
        .unwrap()
        .plan
        .textures()
        .iter()
        .any(|texture| texture.desc.domain.data_rect == lumen_gpu::RectI::new(2, 0, 13, 4));
    assert!(
        bounded,
        "the square's texture covers its path over the clip's frames"
    );
}

/// A 16 × 4 frame with a 2 × 2 white square at (`x`, 1).
fn render_shape_like(x: usize) -> Vec<u8> {
    let mut pixels = vec![0; 16 * 4 * 4];
    for y in 1..3 {
        for x in x..x + 2 {
            pixels[(y * 16 + x) * 4..][..4].copy_from_slice(&[255; 4]);
        }
    }
    pixels
}

/// A 32 × 8 root playing the template `small` with `clip` params, where `small` draws `nodes`
/// (ids 1 and 2) on an 8 × 8 canvas.
fn small_template_composition(nodes: serde_json::Value) -> serde_json::Value {
    json!({
        "timeline": { "fps": 30, "duration_frames": 30 },
        "render_settings": { "width": 32, "height": 8 },
        "compositions": {
            "small": {
                "render_settings": { "width": 8, "height": 8 },
                "nodes": nodes,
                "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
            }
        },
        "nodes": [
            { "id": 1, "type": "clip", "params": { "composition": "small" } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    })
}

#[test]
fn template_output_is_cropped_to_its_own_canvas() {
    let composition = lumen_engine::json::parse_value(&small_template_composition(json!([
        { "id": 1, "type": "background", "params": { "width": 16, "height": 8, "paint": "#ff0000" } },
        { "id": 2, "type": "media_output" }
    ])))
    .unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    assert_eq!(&pixels[7 * 4..8 * 4], [255, 0, 0, 255]);
    assert_eq!(
        &pixels[8 * 4..9 * 4],
        CLEAR,
        "drawn past the template's canvas"
    );
}

#[test]
fn clip_definition_edits_rebuild_the_plan() {
    let mut mover = small_template_composition(json!([
        { "id": 1, "type": "shape", "params": {
            "width": 2, "height": 2, "position": "=vec2(prop('x') + frame, 1)",
            "fill_paint": "#ffffff", "edge_antialias": false
        } },
        { "id": 2, "type": "media_output" }
    ]));
    mover["compositions"]["small"]["render_settings"] = json!({ "width": 32, "height": 8 });
    mover["compositions"]["small"]["props"] = json!({ "x": { "type": "number" } });
    mover["nodes"][0]["params"] =
        json!({ "composition": "small", "duration": 3, "props": { "x": 2 } });
    let mut colors = small_template_composition(json!([
        { "id": 1, "type": "background", "params": { "paint": "#ff0000" } },
        { "id": 2, "type": "media_output" }
    ]));
    colors["compositions"]["blue"] = colors["compositions"]["small"].clone();
    colors["compositions"]["blue"]["nodes"][0]["params"]["paint"] = json!("#0000ff");

    let edits: [(serde_json::Value, fn(&mut ClipParamsDelegate)); 3] = [
        (mover.clone(), |params| {
            params.props =
                Deferred::value(ClipProps::from_json_value(&json!({ "x": 20 }), "props").unwrap());
        }),
        (mover, |params| params.offset = Deferred::value(20.0)),
        (colors, |params| {
            params.composition = Deferred::value("blue".into())
        }),
    ];
    for (composition, edit) in edits {
        let mut composition = lumen_engine::json::parse_value(&composition).unwrap();
        let Some(mut renderer) = gpu_composition_renderer() else {
            return;
        };
        render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
        let Some(NodeKind::Clip(clip)) = composition.graph.nodes.get_mut(&NodeId::new(1)) else {
            unreachable!("node 1 is the clip");
        };
        edit(&mut clip.params);
        let edited = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
        let mut fresh = gpu_composition_renderer().unwrap();
        let expected = render_rgba8(&mut fresh, &composition, 0, &EmptyMediaStore);
        assert!(expected.iter().any(|value| *value > 0));
        assert!(
            edited == expected,
            "an edited clip rendered with a stale plan"
        );
        assert_eq!(renderer.plan_compile_count(), 2);
    }
}

/// A video whose frame `n` is a 1 × 1 pixel with red `n`.
#[derive(Debug)]
struct NumberedVideo;

impl MediaStore for NumberedVideo {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        Some(Box::new(NumberedVideo))
    }
}

impl VideoFrameResolver for NumberedVideo {
    fn id(&self) -> &str {
        "video"
    }

    fn metadata(&self) -> VideoMetadata {
        VideoMetadata {
            width: 1,
            height: 1,
            frame_count: 300,
            fps: 30.0,
        }
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, lumen_engine::error::MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::new(CpuMediaFrame {
            rgba: Arc::new(vec![frame as u8, 0, 0, 255]),
            width: 1,
            height: 1,
            row_bytes: 4,
        })))
    }
}

#[test]
fn clip_frame_rate_round_trips_land_on_the_source_frame() {
    // 30 fps root -> 24 fps template -> 30 fps video, so root frame n shows video frame n.
    let mut composition = small_template_composition(json!([
        { "id": 1, "type": "media_in", "params": { "kind": "video", "source": "video" } },
        { "id": 2, "type": "media_output" }
    ]));
    composition["timeline"]["duration_frames"] = json!(300);
    composition["render_settings"] = json!({ "width": 1, "height": 1 });
    composition["compositions"]["small"]["render_settings"] = json!({ "width": 1, "height": 1 });
    composition["compositions"]["small"]["timeline"] = json!({ "fps": 24, "duration_frames": 240 });
    let composition = lumen_engine::json::parse_value(&composition).unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    for frame in 0..250 {
        let required = collect_frame_requirements(&composition, &NumberedVideo, frame).unwrap();
        assert_eq!(required.videos[0].frames, [frame], "predicted at {frame}");
        let pixels = render_rgba8(&mut renderer, &composition, frame, &NumberedVideo);
        assert_eq!(pixels[0], frame as u8, "rendered at {frame}");
    }
}

/// A `fps[0]` root playing a `fps[1]` template that plays a `fps[2]` template with the 30 fps
/// video, all at default timing.
fn nested_video_composition(fps: [u32; 3]) -> lumen_engine::composition::Composition {
    let template = |fps: u32, nodes: serde_json::Value| {
        json!({
            "timeline": { "fps": fps, "duration_frames": 600 },
            "render_settings": { "width": 1, "height": 1 },
            "nodes": nodes,
            "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
        })
    };
    let clip = |composition: &str| {
        json!([
            { "id": 1, "type": "clip", "params": { "composition": composition } },
            { "id": 2, "type": "media_output" }
        ])
    };
    let mut composition = template(fps[0], clip("middle"));
    composition["compositions"] = json!({
        "middle": template(fps[1], clip("inner")),
        "inner": template(fps[2], json!([
            { "id": 1, "type": "media_in", "params": { "kind": "video", "source": "video" } },
            { "id": 2, "type": "media_output" }
        ])),
    });
    lumen_engine::json::parse_value(&composition).unwrap()
}

fn assert_nested_video_frames(fps: [u32; 3], frames: u32, source: impl Fn(u32) -> u32) {
    let composition = nested_video_composition(fps);
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    for frame in 0..frames {
        let required = collect_frame_requirements(&composition, &NumberedVideo, frame).unwrap();
        assert_eq!(
            required.videos[0].frames,
            [source(frame)],
            "predicted at {frame}"
        );
        let pixels = render_rgba8(&mut renderer, &composition, frame, &NumberedVideo);
        assert_eq!(pixels[0], source(frame) as u8, "rendered at {frame}");
    }
}

#[test]
fn nested_clips_at_other_frame_rates_land_on_the_source_frame() {
    // 30 -> 24 -> 60 fps -> 30 fps video: root frame n shows video frame n.
    assert_nested_video_frames([30, 24, 60], 250, |frame| frame);
    // 25 -> 30 -> 24 fps -> 30 fps video: root frame n shows video frame 6n / 5.
    assert_nested_video_frames([25, 30, 24], 200, |frame| frame * 6 / 5);
}

#[test]
fn clip_props_reading_other_nodes_do_not_bound_the_template() {
    let mut composition = small_template_composition(json!([
        { "id": 1, "type": "shape", "params": {
            "width": 2, "height": 2, "position": "=vec2(prop('x'), 1)",
            "fill_paint": "#ffffff", "edge_antialias": false
        } },
        { "id": 2, "type": "media_output" }
    ]));
    composition["compositions"]["small"]["render_settings"] = json!({ "width": 32, "height": 8 });
    composition["compositions"]["small"]["props"] = json!({ "x": { "type": "number" } });
    composition["nodes"][0]["params"] =
        json!({ "composition": "small", "props": { "x": "=node(99, 'width')" } });
    composition["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": 99, "type": "shape", "params": { "width": 2 } }));
    let mut composition = lumen_engine::json::parse_value(&composition).unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    let Some(NodeKind::Shape(shape)) = composition.graph.nodes.get_mut(&NodeId::new(99)) else {
        unreachable!("node 99 is the shape");
    };
    shape.params.width = Deferred::value(20);
    let edited = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    let expected = render_rgba8(
        &mut gpu_composition_renderer().unwrap(),
        &composition,
        0,
        &EmptyMediaStore,
    );
    assert!(expected.iter().any(|value| *value > 0));
    assert!(
        edited == expected,
        "the template was bounded before node 99 changed"
    );
}

#[test]
fn looped_time_remap_targets_stay_inside_nested_clip_windows() {
    let composition = lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 30, "duration_frames": 30 },
        "render_settings": { "width": 1, "height": 1 },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paint": "#ff0000" } },
            { "id": 2, "type": "clip", "params": { "duration": 4 } },
            { "id": 3, "type": "clip", "params": { "duration": 2, "speed": 2 } },
            { "id": 4, "type": "time_remap", "params": {
                "frame": "=1 / 49 * 49", "loop_enabled": true, "loop_start": 1, "loop_end": 2
            } },
            { "id": 5, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 2, "to_port": "source" },
            { "from_node": 2, "to_node": 3, "to_port": "source" },
            { "from_node": 3, "to_node": 4, "to_port": "source" },
            { "from_node": 4, "to_node": 5, "to_port": "source" }
        ]
    }))
    .unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let pixels = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    assert_eq!(pixels, [255, 0, 0, 255]);
}

const CACHE_CANVAS: (u32, u32) = (32, 16);

fn canvas_background(id: NodeId, color: [u8; 4]) -> NodeKind {
    NodeKind::Background(Background {
        id,
        params: BackgroundParamsDelegate {
            paint: Paint::solid(color).into(),
            width: Deferred::value(CACHE_CANVAS.0),
            height: Deferred::value(CACHE_CANVAS.1),
            paint_supersample: Deferred::value(true),
        },
    })
}

fn square(id: NodeId, position: Deferred<(f64, f64)>, size: i64, color: [u8; 4]) -> NodeKind {
    NodeKind::Shape(Shape {
        id,
        params: ShapeParamsDelegate {
            position,
            width: Deferred::value(size),
            height: Deferred::value(size),
            fill_paint: Paint::solid(color).into(),
            ..Default::default()
        },
    })
}

fn multimerge(id: NodeId, layers: &[u64]) -> NodeKind {
    NodeKind::RasterMultiMerge(RasterMultiMerge {
        id,
        layers: layers
            .iter()
            .map(|layer| PortRef::new(NodeId::new(*layer), "output".to_string()))
            .collect(),
        ..Default::default()
    })
}

fn output_node(id: NodeId, source: u64) -> NodeKind {
    NodeKind::MediaOutput(MediaOutput {
        id,
        source: PortRef::new(NodeId::new(source), "output".to_string()),
    })
}

fn cache_composition(nodes: Vec<NodeKind>) -> Composition {
    let mut graph = Graph::new();
    for node in nodes {
        graph.nodes.insert(node.id(), node);
    }
    let mut composition = test_composition(graph);
    composition.render_settings.width = CACHE_CANVAS.0;
    composition.render_settings.height = CACHE_CANVAS.1;
    composition
}

/// A static layer (node 3: a background and a square) under a square moving with the frame.
fn static_under_animated(background: [u8; 4]) -> Composition {
    cache_composition(vec![
        canvas_background(NodeId::new(1), background),
        square(
            NodeId::new(2),
            Deferred::value((4.0, 4.0)),
            8,
            [20, 40, 220, 255],
        ),
        multimerge(NodeId::new(3), &[1, 2]),
        square(
            NodeId::new(4),
            expression("vec2(frame * 2, 6)"),
            4,
            [30, 220, 60, 255],
        ),
        multimerge(NodeId::new(5), &[3, 4]),
        output_node(NodeId::new(6), 5),
    ])
}

/// Renders `frame` without any output kept from earlier frames.
fn uncached_rgba8(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    composition: &Composition,
    frame: u32,
    media: &impl MediaStore,
) -> Vec<u8> {
    renderer.release_resources();
    render_rgba8(renderer, composition, frame, media)
}

#[test]
fn static_subgraph_dispatches_once_and_matches_uncached_frames() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let composition = static_under_animated([200, 30, 30, 255]);
    let mut cached = Vec::new();
    let mut frames = Vec::new();
    for frame in 0..4 {
        let pixels = render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore);
        let expected = uncached_rgba8(&mut reference, &composition, frame, &EmptyMediaStore);
        assert!(
            pixels == expected,
            "frame {frame} differs from an uncached render"
        );
        cached.push(renderer.cached_pass_count());
        frames.push(pixels);
    }
    let per_frame = cached[1];
    assert!(
        per_frame >= 3,
        "the background, square and their merge are kept"
    );
    assert_eq!(cached, [0, per_frame, 2 * per_frame, 3 * per_frame]);
    assert!(frames[0] != frames[3], "the animated square still moves");
}

const MEMO_FRAME_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = vec4<f32>(f32(lumen.frame) / 255.0, lumen.time, 0.5, 1.0);
    textureStore(output, vec2<i32>(id.xy), color);
}
";

/// A frame-dependent shader, read through a memo keyed by `key` when there is one.
fn memo_shader_composition(key: Option<&str>) -> Composition {
    let (shader, memo, output, input) = (
        NodeId::new(1),
        NodeId::new(2),
        NodeId::new(3),
        NodeId::new(4),
    );
    let mut nodes = vec![
        canvas_background(input, [0, 0, 0, 255]),
        wgsl_shader_node(
            shader,
            Deferred::value(MEMO_FRAME_SHADER.to_string()),
            vec![PortRef::new(input, "output".to_string())],
        ),
        output_node(output, if key.is_some() { 2 } else { 1 }),
    ];
    if let Some(key) = key {
        nodes.push(NodeKind::Memo(Memo {
            id: memo,
            params: MemoParamsDelegate {
                key: expression(key),
            },
            source: PortRef::new(shader, "output".to_string()),
        }));
    }
    cache_composition(nodes)
}

#[test]
fn memo_key_renders_its_source_only_when_the_key_changes() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let memoized = memo_shader_composition(Some("floor(frame / 4)"));
    let plain = memo_shader_composition(None);
    let mut skipped = Vec::new();
    let mut frames = Vec::new();
    for frame in 0..9 {
        let before = renderer.cached_pass_count();
        let pixels = render_rgba8(&mut renderer, &memoized, frame, &EmptyMediaStore);
        skipped.push(renderer.cached_pass_count() - before);
        let expected = uncached_rgba8(&mut reference, &plain, frame - frame % 4, &EmptyMediaStore);
        assert!(
            pixels == expected,
            "frame {frame} shows the shader at frame {}",
            frame - frame % 4
        );
        frames.push(pixels);
    }
    // Refreshes skip only the shader's static input; held frames skip the shader and the
    // memo's copy as well.
    let (refresh, held) = (skipped[4], skipped[1]);
    assert_eq!(held, refresh + 2);
    assert_eq!(
        skipped,
        [0, held, held, held, refresh, held, held, held, refresh]
    );
    assert!(frames[3] != frames[4]);
}

#[test]
fn memo_key_is_evaluated_at_fractional_frame_times() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let memoized = memo_shader_composition(Some("floor(frame / 4)"));
    let plain = memo_shader_composition(None);
    let at = |renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
              composition: &Composition,
              frame: f64| {
        let raster = renderer
            .render_frame_at(composition, frame, &EmptyMediaStore)
            .unwrap();
        read_rgba8(renderer.gpu_renderer(), raster)
    };
    let first = at(&mut renderer, &memoized, 3.5);
    assert!(first == at(&mut reference, &plain, 3.5));
    assert!(
        at(&mut reference, &plain, 3.75) != first,
        "the shader animates"
    );
    assert!(
        at(&mut renderer, &memoized, 3.75) == first,
        "the key is still 0 at frame 3.75"
    );
    assert!(at(&mut renderer, &memoized, 4.25) == at(&mut reference, &plain, 4.25));
}

#[test]
fn kept_text_is_drawn_again_after_its_font_loads() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let store = ScaledFontStore::new("Cachey", 2);
    let mut composition = store.composition(0.0, true);
    let output = composition.graph.nodes.remove(&NodeId::new(2)).unwrap();
    let NodeKind::MediaOutput(output) = output else {
        unreachable!();
    };
    composition.graph.nodes.insert(
        NodeId::new(3),
        square(
            NodeId::new(3),
            expression("vec2(frame * 4, 0)"),
            4,
            [255, 255, 255, 255],
        ),
    );
    composition
        .graph
        .nodes
        .insert(NodeId::new(4), multimerge(NodeId::new(4), &[1, 3]));
    composition
        .graph
        .nodes
        .insert(output.id, output_node(output.id, 4));

    let unloaded = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
    render_rgba8(&mut renderer, &composition, 1, &EmptyMediaStore);
    let loaded = render_rgba8(&mut renderer, &composition, 2, &store);
    assert!(loaded != unloaded, "the text is drawn in the loaded font");
    assert!(loaded == uncached_rgba8(&mut reference, &composition, 2, &store));
}

/// Static layers 4 and 5 sharing square 3, alternately selected every two frames, under a moving
/// square.
fn switching_static_layers() -> Composition {
    cache_composition(vec![
        canvas_background(NodeId::new(1), [200, 30, 30, 255]),
        canvas_background(NodeId::new(2), [30, 200, 30, 255]),
        square(
            NodeId::new(3),
            Deferred::value((4.0, 4.0)),
            8,
            [255, 255, 255, 255],
        ),
        multimerge(NodeId::new(4), &[1, 3]),
        multimerge(NodeId::new(5), &[2, 3]),
        NodeKind::Switch(Switch {
            id: NodeId::new(6),
            params: SwitchParamsDelegate {
                selected_layer: expression("floor(frame / 2) % 2"),
            },
            layers: vec![
                PortRef::new(NodeId::new(4), "output".to_string()),
                PortRef::new(NodeId::new(5), "output".to_string()),
            ],
        }),
        square(
            NodeId::new(7),
            expression("vec2(frame * 2, 6)"),
            4,
            [20, 40, 220, 255],
        ),
        multimerge(NodeId::new(8), &[6, 7]),
        output_node(NodeId::new(9), 8),
    ])
}

#[test]
fn kept_output_survives_plan_switches_and_is_dropped_with_evicted_plans() {
    let Some(mut reference) = gpu_composition_renderer() else {
        return;
    };
    let composition = switching_static_layers();
    for budget in [None, Some(0)] {
        let mut renderer = gpu_composition_renderer().unwrap();
        if let Some(budget) = budget {
            renderer.set_plan_cache_budget(budget);
        }
        for frame in 0..8 {
            let pixels = render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore);
            let expected = uncached_rgba8(&mut reference, &composition, frame, &EmptyMediaStore);
            assert!(pixels == expected, "frame {frame} with budget {budget:?}");
        }
        assert!(renderer.cached_pass_count() > 0);
        if budget.is_some() {
            assert!(
                renderer.cached_plan_count() <= 2,
                "only the current and previous plans stay"
            );
        }
    }
}

#[test]
fn replacing_a_kept_texture_renders_it_again() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let composition = static_under_animated([200, 30, 30, 255]);
    for frame in 0..2 {
        render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore);
    }
    let kept = renderer
        .compiled()
        .unwrap()
        .node_outputs
        .get(&PortRef::new(NodeId::new(3), "output".to_string()))
        .unwrap()
        .clone()
        .into_raster(NodeId::new(3), "output")
        .unwrap();
    let gpu = renderer.gpu_renderer_mut();
    let desc = gpu.texture_desc(kept.texture).unwrap();
    let blank = gpu
        .device
        .create_texture(&lumen_gpu::wgpu::TextureDescriptor {
            label: Some("blank"),
            size: desc.domain.storage_size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: lumen_gpu::wgpu::TextureDimension::D2,
            format: desc.format,
            usage: desc.usage,
            view_formats: &[],
        });
    gpu.replace_texture_discard_old(kept.texture, blank, desc)
        .unwrap();
    let pixels = render_rgba8(&mut renderer, &composition, 2, &EmptyMediaStore);
    assert!(pixels == uncached_rgba8(&mut reference, &composition, 2, &EmptyMediaStore));
}

#[test]
fn discarded_binds_keep_nothing_and_stale_uploads_are_still_rejected() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let mut composition = static_under_animated([200, 30, 30, 255]);
    composition.graph.nodes.insert(
        NodeId::new(2),
        NodeKind::Text(Text {
            id: NodeId::new(2),
            params: TextParamsDelegate {
                content: Deferred::value("Kept".to_string()),
                font_size: Deferred::value(12.0),
                position: Deferred::value((2.0, 8.0)),
                ..Default::default()
            },
        }),
    );
    let submit = |renderer: &mut lumen_engine::gpu::GpuCompositionRenderer, frame| {
        let bound = renderer
            .bind_frame(&composition, frame, &EmptyMediaStore)
            .unwrap();
        let (raster, _) = renderer.submit_bound_frame(&bound).unwrap();
        read_rgba8(renderer.gpu_renderer(), raster)
    };
    for (discarded, submitted) in [(0, 1), (2, 3)] {
        renderer
            .bind_frame(&composition, discarded, &EmptyMediaStore)
            .unwrap();
        let pixels = submit(&mut renderer, submitted);
        assert!(
            pixels == uncached_rgba8(&mut reference, &composition, submitted, &EmptyMediaStore),
            "frame {submitted} after discarding a bind of frame {discarded}"
        );
    }
    assert!(renderer.cached_pass_count() > 0);

    let older = renderer
        .bind_frame(&composition, 4, &EmptyMediaStore)
        .unwrap();
    let current = renderer
        .bind_frame(&composition, 5, &EmptyMediaStore)
        .unwrap();
    assert!(renderer.upload_bound_frame(&older).is_err());
    renderer.upload_bound_frame(&current).unwrap();
    let (raster, _) = renderer.submit_render().unwrap();
    assert!(
        read_rgba8(renderer.gpu_renderer(), raster)
            == uncached_rgba8(&mut reference, &composition, 5, &EmptyMediaStore)
    );
}

#[test]
fn kept_output_reaches_external_output_textures() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let fully_static = cache_composition(vec![
        canvas_background(NodeId::new(1), [200, 30, 30, 255]),
        square(
            NodeId::new(2),
            Deferred::value((4.0, 4.0)),
            8,
            [20, 40, 220, 255],
        ),
        multimerge(NodeId::new(3), &[1, 2]),
        output_node(NodeId::new(4), 3),
    ]);
    let memo_root = {
        let mut composition = static_under_animated([200, 30, 30, 255]);
        composition.graph.nodes.insert(
            NodeId::new(7),
            NodeKind::Memo(Memo {
                id: NodeId::new(7),
                params: MemoParamsDelegate {
                    key: expression("floor(frame / 2)"),
                },
                source: PortRef::new(NodeId::new(5), "output".to_string()),
            }),
        );
        composition
            .graph
            .nodes
            .insert(NodeId::new(6), output_node(NodeId::new(6), 7));
        composition
    };
    for (composition, held) in [(&fully_static, false), (&memo_root, true)] {
        renderer.release_resources();
        render_rgba8(&mut renderer, composition, 0, &EmptyMediaStore);
        let output = renderer.compiled().unwrap().output;
        let desc = renderer
            .gpu_renderer()
            .texture_desc(output.texture)
            .unwrap();
        for frame in 1..4 {
            let gpu = renderer.gpu_renderer();
            let texture = gpu
                .device
                .create_texture(&lumen_gpu::wgpu::TextureDescriptor {
                    label: Some("external output"),
                    size: desc.domain.storage_size.as_extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: lumen_gpu::wgpu::TextureDimension::D2,
                    format: desc.format,
                    usage: desc.usage | lumen_gpu::wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
            let submitted = renderer
                .render_frame_into_external(
                    composition,
                    frame,
                    &EmptyMediaStore,
                    lumen_gpu::ExternalTexture::from_texture(texture, desc),
                )
                .unwrap();
            submitted.wait(&renderer.gpu_renderer().device).unwrap();
            let pixels = read_texture_rgba8(
                renderer.gpu_renderer(),
                submitted.texture(),
                desc.domain.storage_size,
            );
            let shown = if held { frame - frame % 2 } else { frame };
            let mut expected_composition = composition;
            let plain;
            if held {
                plain = static_under_animated([200, 30, 30, 255]);
                expected_composition = &plain;
            }
            let expected = uncached_rgba8(
                &mut reference,
                expected_composition,
                shown,
                &EmptyMediaStore,
            );
            assert!(
                pixels == expected,
                "external frame {frame} (memo root: {held})"
            );
        }
        assert!(renderer.cached_pass_count() > 0);
    }
}

#[test]
fn kept_text_keeps_its_glyphs_in_the_shared_atlas() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let raster_text = |id: u64, content: &str, font_size: Deferred<f64>, x: f64| {
        NodeKind::Text(Text {
            id: NodeId::new(id),
            params: TextParamsDelegate {
                content: Deferred::value(content.to_string()),
                font_size,
                scale: Deferred::value((0.03, 0.03)),
                position: Deferred::value((x, 0.0)),
                render_mode: lumen_engine::node::source::text::TextRenderMode::Raster.into(),
                ..Default::default()
            },
        })
    };
    // Glyphs this large fill most of the atlas, so the static "W" and the "M" that grows on
    // frame 1 cannot both fit; the "W" is drawn first, so it keeps its glyph.
    let mut composition = cache_composition(vec![
        raster_text(1, "W", Deferred::value(3800.0), 0.0),
        raster_text(2, "M", expression("12 + 3788 * min(frame, 1)"), 120.0),
        multimerge(NodeId::new(3), &[1, 2]),
        output_node(NodeId::new(4), 3),
    ]);
    composition.render_settings.width = 256;
    composition.render_settings.height = 128;
    for frame in 0..3 {
        let pixels = render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore);
        let expected = uncached_rgba8(&mut reference, &composition, frame, &EmptyMediaStore);
        assert!(
            pixels == expected,
            "frame {frame} differs from an uncached render"
        );
    }
    assert!(renderer.cached_pass_count() > 0, "the static text is kept");
}

#[test]
fn memo_snapshot_is_shared_by_the_plans_its_key_spans() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let at = |renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
              composition: &Composition,
              frame: f64| {
        let raster = renderer
            .render_frame_at(composition, frame, &EmptyMediaStore)
            .unwrap();
        read_rgba8(renderer.gpu_renderer(), raster)
    };
    // A fractional frame binds another plan than whole frames do.
    let memoized = memo_shader_composition(Some("floor(frame / 100)"));
    let first = at(&mut renderer, &memoized, 0.0);
    assert!(at(&mut renderer, &memoized, 0.5) == first, "frame 0.5");
    assert!(at(&mut renderer, &memoized, 1.0) == first, "frame 1");
    assert!(renderer.cached_plan_count() >= 2);

    // Whole frames size a moving square to its bounds over the timeline; fractional ones don't.
    renderer.release_resources();
    let moving = cache_composition(vec![
        square(
            NodeId::new(1),
            expression("vec2(frame * 4, 4)"),
            4,
            [255, 0, 0, 255],
        ),
        NodeKind::Memo(Memo {
            id: NodeId::new(2),
            params: MemoParamsDelegate {
                key: expression("floor(frame / 100)"),
            },
            source: PortRef::new(NodeId::new(1), "output".to_string()),
        }),
        output_node(NodeId::new(3), 2),
    ]);
    let first = at(&mut renderer, &moving, 0.0);
    assert!(at(&mut renderer, &moving, 0.5) == first, "moving frame 0.5");
    assert!(at(&mut renderer, &moving, 1.0) == first, "moving frame 1");

    // A switch upstream of the memo selects another plan every two frames.
    renderer.release_resources();
    let mut switched = memo_shader_composition(Some("floor(frame / 100)"));
    switched.graph.nodes.insert(
        NodeId::new(5),
        canvas_background(NodeId::new(5), [0, 255, 0, 255]),
    );
    switched.graph.nodes.insert(
        NodeId::new(6),
        NodeKind::Switch(Switch {
            id: NodeId::new(6),
            params: SwitchParamsDelegate {
                selected_layer: expression("floor(frame / 2) % 2"),
            },
            layers: vec![
                PortRef::new(NodeId::new(1), "output".to_string()),
                PortRef::new(NodeId::new(5), "output".to_string()),
            ],
        }),
    );
    let NodeKind::Memo(memo) = switched.graph.nodes.get_mut(&NodeId::new(2)).unwrap() else {
        unreachable!();
    };
    memo.source = PortRef::new(NodeId::new(6), "output".to_string());
    let first = render_rgba8(&mut renderer, &switched, 0, &EmptyMediaStore);
    for frame in 1..6 {
        let pixels = render_rgba8(&mut renderer, &switched, frame, &EmptyMediaStore);
        assert!(pixels == first, "frame {frame} shows the memo's frame 0");
    }
    assert!(renderer.cached_plan_count() >= 2);
}

#[test]
fn memo_holds_sources_that_publish_raster_bounds() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    // A crop moving with the frame, read directly and through a resize, which reads its bounds.
    let composition = |memo: bool, resize: bool| {
        let mut nodes = vec![
            canvas_background(NodeId::new(1), [255, 0, 0, 255]),
            square(
                NodeId::new(2),
                Deferred::value((6.0, 6.0)),
                6,
                [20, 40, 220, 255],
            ),
            multimerge(NodeId::new(3), &[1, 2]),
            NodeKind::Crop(Crop {
                id: NodeId::new(4),
                params: CropParamsDelegate {
                    x: expression("frame * 4"),
                    width: Deferred::value(8),
                    height: Deferred::value(8),
                    ..Default::default()
                },
                source: PortRef::new(NodeId::new(3), "output".to_string()),
            }),
        ];
        let mut last = 4;
        if memo {
            nodes.push(NodeKind::Memo(Memo {
                id: NodeId::new(5),
                params: MemoParamsDelegate {
                    key: expression("floor(frame / 100)"),
                },
                source: PortRef::new(NodeId::new(last), "output".to_string()),
            }));
            last = 5;
        }
        if resize {
            nodes.push(NodeKind::Resize(Resize {
                id: NodeId::new(6),
                params: ResizeParamsDelegate {
                    width: Deferred::value(i64::from(CACHE_CANVAS.0)),
                    height: Deferred::value(i64::from(CACHE_CANVAS.1)),
                    mode: ResizeMode::Fit.into(),
                    sampling: ResizeSampling::Nearest.into(),
                    ..Default::default()
                },
                source: PortRef::new(NodeId::new(last), "output".to_string()),
            }));
            last = 6;
        }
        nodes.push(output_node(NodeId::new(7), last));
        cache_composition(nodes)
    };
    for resize in [false, true] {
        renderer.release_resources();
        let memoized = composition(true, resize);
        let expected = uncached_rgba8(
            &mut reference,
            &composition(false, resize),
            0,
            &EmptyMediaStore,
        );
        assert!(
            expected
                != uncached_rgba8(
                    &mut reference,
                    &composition(false, resize),
                    1,
                    &EmptyMediaStore
                ),
            "the crop moves"
        );
        for frame in 0..3 {
            let pixels = render_rgba8(&mut renderer, &memoized, frame, &EmptyMediaStore);
            assert!(
                pixels == expected,
                "frame {frame} (resized: {resize}) shows the crop at frame 0"
            );
        }
    }
}

fn render_rgba8_at(
    renderer: &mut lumen_engine::gpu::GpuCompositionRenderer,
    composition: &Composition,
    frame: f64,
) -> Vec<u8> {
    let raster = renderer
        .render_frame_at(composition, frame, &EmptyMediaStore)
        .unwrap();
    read_rgba8(renderer.gpu_renderer(), raster)
}

fn keyed_memo(id: u64, source: u64, key: &str) -> NodeKind {
    NodeKind::Memo(Memo {
        id: NodeId::new(id),
        params: MemoParamsDelegate {
            key: expression(key),
        },
        source: PortRef::new(NodeId::new(source), "output".to_string()),
    })
}

#[test]
fn memo_snapshots_do_not_survive_composition_edits() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut composition = cache_composition(vec![
        square(
            NodeId::new(99),
            Deferred::value((0.0, 0.0)),
            4,
            [0, 255, 0, 255],
        ),
        square(
            NodeId::new(1),
            expression("vec2(node(99, 'width'), 0)"),
            4,
            [255, 0, 0, 255],
        ),
        keyed_memo(2, 1, "0 + frame * 0"),
        output_node(NodeId::new(3), 2),
    ]);
    let before = render_rgba8_at(&mut renderer, &composition, 0.0);
    let NodeKind::Shape(shape) = composition.graph.nodes.get_mut(&NodeId::new(99)).unwrap() else {
        unreachable!();
    };
    shape.params.width = Deferred::value(16);
    let mut fresh = gpu_composition_renderer().unwrap();
    let expected = render_rgba8_at(&mut fresh, &composition, 0.0);
    assert!(expected != before, "the edit moves the square");
    // The existing plan, then a plan compiled for a fractional frame.
    for frame in [1.0, 0.5] {
        assert!(
            render_rgba8_at(&mut renderer, &composition, frame) == expected,
            "frame {frame} shows the edited composition"
        );
    }
}

#[test]
fn literal_param_edits_rebuild_the_plan() {
    let edits: [fn(&mut Composition); 2] = [
        |composition| {
            let Some(NodeKind::Shape(shape)) = composition.graph.nodes.get_mut(&NodeId::new(1))
            else {
                unreachable!("node 1 is the square");
            };
            shape.params.position = Deferred::value((20.0, 2.0));
        },
        |composition| {
            let Some(NodeKind::Blur(blur)) = composition.graph.nodes.get_mut(&NodeId::new(2))
            else {
                unreachable!("node 2 is the blur");
            };
            blur.params.radius = Deferred::value(4.0);
        },
    ];
    for edit in edits {
        let Some(mut renderer) = gpu_composition_renderer() else {
            return;
        };
        let mut composition = cache_composition(vec![
            square(
                NodeId::new(1),
                Deferred::value((2.0, 2.0)),
                4,
                [255, 0, 0, 255],
            ),
            NodeKind::Blur(Blur {
                id: NodeId::new(2),
                params: BlurParamsDelegate {
                    radius: Deferred::value(1.0),
                },
                source: PortRef::new(NodeId::new(1), "output".to_string()),
            }),
            output_node(NodeId::new(3), 2),
        ]);
        let before = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
        edit(&mut composition);
        let edited = render_rgba8(&mut renderer, &composition, 0, &EmptyMediaStore);
        let mut fresh = gpu_composition_renderer().unwrap();
        let expected = render_rgba8(&mut fresh, &composition, 0, &EmptyMediaStore);
        assert!(expected != before, "the edit changes the frame");
        assert!(
            edited == expected,
            "an edited node rendered with a stale plan"
        );
        assert_eq!(renderer.cached_plan_count(), 1);
    }
}

#[test]
fn memo_snapshot_dropped_with_its_key_change_is_not_shown_again() {
    let _guard = TEXT_TEST_LOCK.lock().unwrap();
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    // Frame 1 places more glyphs than the atlas holds, so its frame is not kept.
    let mut composition = memo_shader_composition(Some("floor(frame) % 2"));
    composition.graph.nodes.insert(
        NodeId::new(5),
        NodeKind::Text(Text {
            id: NodeId::new(5),
            params: TextParamsDelegate {
                content: expression(&format!(
                    "if(floor(frame) % 2 == 1, '{}', 'W')",
                    "W".repeat(3000)
                )),
                font_size: Deferred::value(12.0),
                scale: Deferred::value((0.001, 0.001)),
                position: Deferred::value((30.0, 10.0)),
                render_mode: lumen_engine::node::source::text::TextRenderMode::Raster.into(),
                ..Default::default()
            },
        }),
    );
    composition
        .graph
        .nodes
        .insert(NodeId::new(6), multimerge(NodeId::new(6), &[2, 5]));
    composition
        .graph
        .nodes
        .insert(NodeId::new(3), output_node(NodeId::new(3), 6));
    let plain = memo_shader_composition(None);
    // Frames 0 and 0.5 are bound to different plans, each holding key 0.
    for frame in [0.0, 0.5, 1.0, 2.0] {
        let pixels = render_rgba8_at(&mut renderer, &composition, frame);
        let expected = render_rgba8_at(&mut reference, &plain, frame.floor());
        assert!(
            pixels[..4] == expected[..4],
            "frame {frame} shows the shader at frame {}",
            frame.floor()
        );
    }
}

#[test]
fn plan_cache_budget_keeps_the_plan_a_bound_frame_copies_from() {
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let composition = memo_shader_composition(Some("frame * 0"));
    let first = render_rgba8_at(&mut renderer, &composition, 0.0);
    let bound = renderer
        .bind_frame_at(&composition, 0.5, &EmptyMediaStore)
        .unwrap();
    renderer.set_plan_cache_budget(0);
    let raster = renderer.submit_bound_frame(&bound).unwrap().0;
    assert!(read_rgba8(renderer.gpu_renderer(), raster) == first);
}

/// One or two clips of a `tile` template whose node ids repeat the root's, the second moved to
/// the right half. The template's nodes 1 → 2 are the merge's base and 3 its overlay.
fn tile_clips(clips: serde_json::Value, template_nodes: serde_json::Value) -> Composition {
    let mut nodes = clips.as_array().unwrap().clone();
    let mut connections = vec![json!({ "from_node": 1, "to_node": 5, "to_port": "source" })];
    if nodes.len() == 2 {
        nodes.extend([
            json!({ "id": 3, "type": "transform", "params": { "translate_x": 2 } }),
            json!({ "id": 4, "type": "merge" }),
        ]);
        connections = vec![
            json!({ "from_node": 2, "to_node": 3, "to_port": "source" }),
            json!({ "from_node": 1, "to_node": 4, "to_port": "base" }),
            json!({ "from_node": 3, "to_node": 4, "to_port": "overlay" }),
            json!({ "from_node": 4, "to_node": 5, "to_port": "source" }),
        ];
    }
    nodes.push(json!({ "id": 5, "type": "media_output" }));
    lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 10, "duration_frames": 20 },
        "render_settings": { "width": 4, "height": 1 },
        "compositions": {
            "tile": {
                "render_settings": { "width": 2, "height": 1 },
                "timeline": { "fps": 10, "duration_frames": 20 },
                "props": { "g": { "type": "number", "default": 0 } },
                "nodes": template_nodes,
                "connections": [
                    { "from_node": 1, "to_node": 2, "to_port": "source" },
                    { "from_node": 2, "to_node": 4, "to_port": "base" },
                    { "from_node": 3, "to_node": 4, "to_port": "overlay" },
                    { "from_node": 4, "to_node": 5, "to_port": "source" }
                ]
            }
        },
        "nodes": nodes,
        "connections": connections
    }))
    .unwrap()
}

#[test]
fn static_template_parts_are_kept_per_clip_instance() {
    let template = json!([
        { "id": 1, "type": "background", "params": { "paint": "#2040c0" } },
        { "id": 2, "type": "opacity", "params": { "opacity": 0.5 } },
        { "id": 3, "type": "background", "params": { "paint": "=rgba(frame * 10, prop('g'), 0, 128)" } },
        { "id": 4, "type": "merge" },
        { "id": 5, "type": "media_output" }
    ]);
    let first = json!({ "id": 1, "type": "clip", "params": { "composition": "tile", "props": { "g": 40 } } });
    let second = json!({ "id": 2, "type": "clip", "params": { "composition": "tile", "start": 2, "props": { "g": 200 } } });
    let both = tile_clips(json!([first, second]), template.clone());
    let one = tile_clips(json!([first]), template);
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    let mut reference = gpu_composition_renderer().unwrap();
    let mut kept_per_frame = |composition: &Composition| {
        let mut cached = Vec::new();
        let mut frames = Vec::new();
        for frame in 2..6 {
            let pixels = render_rgba8(&mut renderer, composition, frame, &EmptyMediaStore);
            let expected = uncached_rgba8(&mut reference, composition, frame, &EmptyMediaStore);
            assert!(
                pixels == expected,
                "frame {frame} differs from an uncached render"
            );
            cached.push(renderer.cached_pass_count());
            frames.push(pixels);
        }
        let per_frame = cached[1] - cached[0];
        let start = cached[0];
        assert_eq!(
            cached,
            [
                start,
                start + per_frame,
                start + 2 * per_frame,
                start + 3 * per_frame
            ],
            "one plan keeps the same passes every frame"
        );
        assert!(frames[0] != frames[3], "the animated layer still moves");
        (per_frame, frames)
    };
    let (one_kept, _) = kept_per_frame(&one);
    let (both_kept, frames) = kept_per_frame(&both);
    assert!(one_kept >= 2, "the background and its opacity are kept");
    assert_eq!(
        both_kept,
        2 * one_kept,
        "each clip keeps its own static part"
    );
    assert!(
        frames[2][..8] != frames[2][8..],
        "the clips show their own props and time"
    );
}

#[test]
fn keyed_memo_in_a_template_holds_per_clip_instance() {
    // The memo holds the animated background, under a transparent overlay.
    let composition = tile_clips(
        json!([
            { "id": 1, "type": "clip", "params": { "composition": "tile" } },
            { "id": 2, "type": "clip", "params": { "composition": "tile", "start": 2 } }
        ]),
        json!([
            { "id": 1, "type": "background", "params": { "paint": "=rgb(frame * 10, 0, 0)" } },
            { "id": 2, "type": "memo", "params": { "key": "=floor(frame / 4)" } },
            { "id": 3, "type": "background", "params": { "paint": "=rgba(0, 0, 0, 0)" } },
            { "id": 4, "type": "merge" },
            { "id": 5, "type": "media_output" }
        ]),
    );
    let Some(mut renderer) = gpu_composition_renderer() else {
        return;
    };
    // Each clip shows its local frame rounded down to a multiple of 4.
    let held = |local: u32| (local - local % 4) as u8 * 10;
    let mut skipped = 0;
    for frame in 0..12 {
        let before = renderer.cached_pass_count();
        let pixels = render_rgba8(&mut renderer, &composition, frame, &EmptyMediaStore);
        skipped += renderer.cached_pass_count() - before;
        assert_eq!(pixels[0], held(frame), "first clip at frame {frame}");
        if frame >= 2 {
            assert_eq!(pixels[8], held(frame - 2), "second clip at frame {frame}");
        }
    }
    assert!(skipped > 0, "held memos skip their sources");
}

fn render_held_memo_frames(
    composition: &serde_json::Value,
    frames: &[u32],
) -> Option<Vec<Vec<u8>>> {
    let composition = lumen_engine::json::parse_value(composition).unwrap();
    let mut renderer = gpu_composition_renderer()?;
    Some(
        frames
            .iter()
            .map(|frame| render_rgba8(&mut renderer, &composition, *frame, &EmptyMediaStore))
            .collect(),
    )
}

#[test]
fn memo_holds_cropped_source_when_its_clip_hides() {
    let composition = json!({
        "timeline": {"fps": 10, "duration_frames": 10},
        "render_settings": {"width": 4, "height": 4},
        "nodes": [
            {"id":1,"type":"background","params":{"paint":"#ff0000"}},
            {"id":2,"type":"crop","params":{"width":2,"height":2}},
            {"id":3,"type":"clip","params":{"duration":2}},
            {"id":4,"type":"memo","params":{"key":"=frame * 0"}},
            {"id":5,"type":"media_output"}
        ],
        "connections": [
            {"from_node":1,"to_node":2,"to_port":"source"},
            {"from_node":2,"to_node":3,"to_port":"source"},
            {"from_node":3,"to_node":4,"to_port":"source"},
            {"from_node":4,"to_node":5,"to_port":"source"}
        ]
    });
    let Some(frames) = render_held_memo_frames(&composition, &[0, 2]) else {
        return;
    };
    assert_eq!(&frames[0][..4], &[255, 0, 0, 255]);
    assert_eq!(
        frames[1], frames[0],
        "the unchanged key holds the cropped source"
    );
}

#[test]
fn memo_holds_cropped_template_when_its_clip_hides() {
    let composition = json!({
        "timeline": {"fps": 10, "duration_frames": 10},
        "render_settings": {"width": 4, "height": 4},
        "compositions": {
            "cropped": {
                "timeline": {"fps": 10, "duration_frames": 10},
                "render_settings": {"width": 4, "height": 4},
                "nodes": [
                    {"id":1,"type":"background","params":{"paint":"#ff0000"}},
                    {"id":2,"type":"crop","params":{"width":2,"height":2}},
                    {"id":5,"type":"media_output"}
                ],
                "connections": [
                    {"from_node":1,"to_node":2,"to_port":"source"},
                    {"from_node":2,"to_node":5,"to_port":"source"}
                ]
            }
        },
        "nodes": [
            {"id":3,"type":"clip","params":{"composition":"cropped","duration":2}},
            {"id":4,"type":"memo","params":{"key":"=frame * 0"}},
            {"id":5,"type":"media_output"}
        ],
        "connections": [
            {"from_node":3,"to_node":4,"to_port":"source"},
            {"from_node":4,"to_node":5,"to_port":"source"}
        ]
    });
    let Some(frames) = render_held_memo_frames(&composition, &[0, 2]) else {
        return;
    };
    assert_eq!(&frames[0][..4], &[255, 0, 0, 255]);
    assert_eq!(
        frames[1], frames[0],
        "the unchanged key holds the cropped template"
    );
}

#[test]
fn memo_holds_snapshot_across_switch_between_uncropped_and_cropped_layers() {
    let composition = json!({
        "timeline": {"fps": 10, "duration_frames": 10},
        "render_settings": {"width": 4, "height": 4},
        "nodes": [
            {"id":1,"type":"background","params":{"paint":"#ff0000"}},
            {"id":2,"type":"crop","params":{"width":2,"height":2}},
            {"id":3,"type":"switch","params":{"selected_layer":"=floor(frame / 2) % 2"}},
            {"id":4,"type":"memo","params":{"key":"=frame * 0"}},
            {"id":5,"type":"media_output"}
        ],
        "connections": [
            {"from_node":1,"to_node":3,"to_port":"layers"},
            {"from_node":2,"to_node":3,"to_port":"layers"},
            {"from_node":1,"to_node":2,"to_port":"source"},
            {"from_node":3,"to_node":4,"to_port":"source"},
            {"from_node":4,"to_node":5,"to_port":"source"}
        ]
    });
    // The uncropped layer is selected first and the cropped one next, and each plan then
    // adopts the snapshot from the other.
    let Some(frames) = render_held_memo_frames(&composition, &[0, 2, 4, 6]) else {
        return;
    };
    assert_eq!(&frames[0][..4], &[255, 0, 0, 255]);
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(
            frame,
            &frames[0],
            "frame {} holds the first snapshot",
            index * 2
        );
    }
}
