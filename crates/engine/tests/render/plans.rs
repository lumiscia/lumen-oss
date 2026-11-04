//! How the renderer keeps, rebuilds and drops compiled plans as a composition is edited or looked
//! ahead of.

use std::collections::HashMap;
use std::sync::Arc;

use lumen_engine::{
    composition::Composition,
    gpu::{CompileContext, CompiledComposition},
    node::NodeId,
};
use serde_json::{Value, json};

use crate::shaders::{fill_shader, shader_scene};
use crate::support::{
    Scene, TestMedia, gpu, read_rgba8, render, render_held, scene, within_rounding,
};

const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn assert_uniform(rgba: &[u8], color: [u8; 4], what: &str) {
    for pixel in rgba.chunks(4) {
        assert_eq!(pixel, color, "{what}");
    }
}

fn plan_color(index: u32) -> [u8; 4] {
    [index as u8 * 25 + 5, 0, 0, 255]
}

/// An 8 × 4 scene that shows `plans` solid layers in turn, so each frame is a plan of its own.
fn switch_scene(plans: u32) -> Scene {
    let mut nodes: Vec<Value> = (0..plans)
        .map(|index| {
            json!({ "id": index + 1, "type": "background", "params": {
                "paint": plan_color(index), "width": 8, "height": 4
            } })
        })
        .collect();
    nodes.push(json!({
        "id": 100, "type": "switch", "in": { "layers": (1..=plans).collect::<Vec<_>>() },
        "params": { "selected_layer": format!("=frame % {plans}") }
    }));
    scene((8, 4), json!(nodes)).frames(120)
}

/// An 8 × 4 scene of `bits` shaders in a row, shader `bit` adding 2^bit to red on alternating runs
/// of 2^bit frames, so it has 2^bits distinct plans.
fn animated_shader_scene(bits: u32) -> Scene {
    let add_red = |red: u32| {
        format!(
            "@compute @workgroup_size(8, 8, 1) fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) \
             {{ let color = textureLoad(input0, vec2<i32>(id.xy), 0); \
             textureStore(output, vec2<i32>(id.xy), color + vec4<f32>({red}.0 / 255.0, 0.0, 0.0, 0.0)); }}"
        )
    };
    let mut nodes = vec![
        json!({ "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255], "width": 8, "height": 4 } }),
    ];
    for bit in 0..bits {
        let toggle = format!(
            "=if(floor(frame / {}) % 2 == 0, '{}', '{}')",
            1 << bit,
            add_red(0),
            add_red(1 << bit)
        );
        nodes.push(json!({
            "id": bit + 2, "type": "wgsl_shader", "in": { "source": bit + 1 },
            "params": { "shader": toggle }
        }));
    }
    scene((8, 4), json!(nodes)).frames(1 << (bits + 1))
}

/// A 4 × 4 red square at (2, 2) blurred by `radius`, on a 32 × 16 canvas.
fn blurred_square(radius: u32) -> Scene {
    scene(
        (32, 16),
        json!([
            { "id": 1, "type": "shape", "params": {
                "position": [2, 2], "width": 4, "height": 4, "fill_paint": [255, 0, 0, 255]
            } },
            { "id": 2, "type": "blur", "in": { "source": 1 }, "params": { "radius": radius } },
        ]),
    )
}

/// A 96 × 16 scene of an 8-high orange shape at `position`.
fn shape_at(position: Value, width: Value) -> Scene {
    scene(
        (96, 16),
        json!([{ "id": 1, "type": "shape", "params": {
            "position": position, "width": width, "height": 8, "fill_paint": [255, 100, 0, 255]
        } }]),
    )
}

/// A root playing the template `small` through a clip with `clip`, where `small` draws `nodes`
/// (ids 1 and 2) on a canvas of the same size.
fn small_template(canvas: (u32, u32), nodes: Value, clip: Value) -> Scene {
    let render_settings = json!({ "width": canvas.0, "height": canvas.1 });
    Scene(json!({
        "timeline": { "fps": 30, "duration_frames": 30 },
        "render_settings": render_settings,
        "compositions": { "small": {
            "render_settings": render_settings,
            "props": { "x": { "type": "number" } },
            "nodes": nodes,
            "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
        } },
        "nodes": [
            { "id": 1, "type": "clip", "params": clip },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    }))
}

/// A 2 × 2 white square at x = `prop('x')` + the template's frame, on a 32 × 4 canvas.
fn moving_square(clip: Value) -> Scene {
    small_template(
        (32, 4),
        json!([
            { "id": 1, "type": "shape", "params": {
                "width": 2, "height": 2, "position": "=vec2(prop('x') + frame, 1)",
                "fill_paint": "#ffffff", "edge_antialias": false
            } },
            { "id": 2, "type": "media_output" }
        ]),
        clip,
    )
}

struct Edit {
    name: &'static str,
    scene: Scene,
    apply: fn(&mut Value),
    frame: f64,
    /// A pixel that is drawn once the edit is, and not before it.
    probe: (u32, u32),
}

#[test]
fn edits_render_like_a_fresh_renderer() {
    let colored = |color: &str| {
        json!([
            { "id": 1, "type": "background", "params": { "paint": color } },
            { "id": 2, "type": "media_output" }
        ])
    };
    let mut blue_template = small_template(
        (8, 4),
        colored("#ff0000"),
        json!({ "composition": "small" }),
    );
    blue_template.0["compositions"]["blue"] = blue_template.0["compositions"]["small"].clone();
    blue_template.0["compositions"]["blue"]["nodes"][0]["params"]["paint"] = json!("#0000ff");
    let mut node_read =
        moving_square(json!({ "composition": "small", "props": { "x": "=node(99, 'width')" } }));
    node_read.0["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": 99, "type": "shape", "params": { "width": 2 } }));
    let edits = [
        Edit {
            name: "timeline duration",
            scene: shape_at(json!("=vec2(frame * 10, 4)"), json!(8))
                .frames(2)
                .fps(24.0),
            apply: |scene| scene["timeline"]["duration_frames"] = json!(10),
            frame: 8.0,
            probe: (84, 8),
        },
        Edit {
            name: "timeline fps",
            scene: shape_at(json!("=vec2(time * 100, 4)"), json!(8))
                .frames(10)
                .fps(24.0),
            apply: |scene| scene["timeline"]["fps"] = json!(12),
            frame: 8.0,
            probe: (70, 8),
        },
        Edit {
            name: "a width read from a node off the output",
            scene: {
                let mut scene = shape_at(json!([10, 4]), json!("=node(99, 'width')"));
                scene.0["nodes"].as_array_mut().unwrap().insert(
                    0,
                    json!({ "id": 99, "type": "shape", "params": { "width": 8 } }),
                );
                scene
            },
            apply: |scene| scene["nodes"][0]["params"]["width"] = json!(80),
            frame: 0.0,
            probe: (85, 8),
        },
        Edit {
            name: "a literal position",
            scene: blurred_square(1),
            apply: |scene| scene["nodes"][0]["params"]["position"] = json!([20, 2]),
            frame: 0.0,
            probe: (21, 3),
        },
        Edit {
            name: "a literal blur radius",
            scene: blurred_square(1),
            apply: |scene| scene["nodes"][1]["params"]["radius"] = json!(4),
            frame: 0.0,
            probe: (9, 3),
        },
        Edit {
            name: "clip props",
            scene: moving_square(
                json!({ "composition": "small", "duration": 3, "props": { "x": 2 } }),
            ),
            apply: |scene| scene["nodes"][0]["params"]["props"] = json!({ "x": 20 }),
            frame: 0.0,
            probe: (21, 2),
        },
        Edit {
            name: "clip offset",
            scene: moving_square(
                json!({ "composition": "small", "duration": 3, "props": { "x": 2 } }),
            ),
            apply: |scene| scene["nodes"][0]["params"]["offset"] = json!(20),
            frame: 0.0,
            probe: (23, 2),
        },
        Edit {
            name: "clip template",
            scene: blue_template,
            apply: |scene| scene["nodes"][0]["params"]["composition"] = json!("blue"),
            frame: 0.0,
            probe: (0, 0),
        },
        Edit {
            name: "a node a clip prop reads",
            scene: node_read,
            apply: |scene| scene["nodes"][2]["params"]["width"] = json!(20),
            frame: 0.0,
            probe: (21, 2),
        },
        Edit {
            name: "canvas size",
            scene: switch_scene(9),
            apply: |scene| scene["render_settings"] = json!({ "width": 16, "height": 8 }),
            frame: 0.0,
            probe: (7, 3),
        },
    ];

    let gpu = gpu();
    let media = TestMedia::default();
    for edit in edits {
        let mut value = edit.scene.0;
        let parse = |value: &Value| Composition::from(Scene(value.clone()));
        let mut renderer = gpu.renderer();
        render(&mut renderer, &parse(&value), 0.0, &media);
        (edit.apply)(&mut value);
        let edited = parse(&value);
        let actual = render(&mut renderer, &edited, edit.frame, &media);
        let fresh = render(&mut gpu.reference(), &edited, edit.frame, &media);

        let (x, y) = edit.probe;
        assert_ne!(
            fresh.pixel(x, y),
            CLEAR,
            "{}: the edit draws nothing at ({x}, {y})",
            edit.name
        );
        if let Err(error) = within_rounding(&actual.rgba, &fresh.rgba) {
            panic!(
                "{}: the edited render differs from a fresh one: {error}",
                edit.name
            );
        }
    }
}

#[test]
fn a_radius_edit_reuses_the_blur_programs() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    render(&mut renderer, &blurred_square(1).into(), 0.0, &media);
    let created = renderer.created_program_count();
    assert!(created > 0);
    render(&mut renderer, &blurred_square(4).into(), 0.0, &media);
    assert_eq!(renderer.created_program_count(), created);
}

fn red_shader(red: u8) -> Composition {
    let color = format!("vec4<f32>({red}.0 / 255.0, 0.0, 0.0, 1.0)");
    Composition::from(shader_scene(&fill_shader(&color), json!({}), &[[0; 4]]))
}

#[test]
fn shader_edits_replace_the_plan_and_keep_held_textures() {
    let gpu = gpu();
    let media = TestMedia::default();
    for precompile_first in [false, true] {
        let mut renderer = gpu.renderer();
        let plans = !renderer.frame_graph_enabled();
        let mut outputs = Vec::new();
        let mut held = None;
        for edit in 0..12u8 {
            let red = (edit + 1) * 20;
            let composition = red_shader(red);
            if precompile_first {
                renderer.precompile_frame(&composition, 0, &media).unwrap();
                if plans {
                    assert_eq!(
                        renderer.cached_plan_count(),
                        if edit == 0 { 1 } else { 2 },
                        "precompiling edit {edit} keeps the plan it supersedes"
                    );
                }
            }
            let (frame, texture) = render_held(&mut renderer, &composition, 0.0, &media);
            assert_uniform(&frame.rgba, [red, 0, 0, 255], &format!("edit {edit}"));
            if plans {
                assert_eq!(
                    renderer.cached_plan_count(),
                    1,
                    "rendering edit {edit} (precompiled first: {precompile_first}) keeps a superseded plan"
                );
            }
            held.get_or_insert_with(|| (texture.as_ref().clone(), frame.width, frame.height));
            outputs.push(Arc::downgrade(&texture));
        }
        let resident = outputs
            .iter()
            .filter(|output| output.strong_count() > 0)
            .count();
        assert_eq!(resident, 1, "superseded plan outputs are still resident");

        let (texture, width, height) = held.unwrap();
        let size = lumen_gpu::Size::new(width, height);
        let pixels = read_rgba8(renderer.gpu_renderer(), &texture, size);
        assert_uniform(
            &pixels,
            [20, 0, 0, 255],
            "the texture held from the first edit",
        );
    }
}

#[test]
fn lookahead_compiles_every_switch_plan_ahead_and_keeps_them() {
    const PLANS: u32 = 9;
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition = Composition::from(switch_scene(PLANS));
    let plans = !renderer.frame_graph_enabled();

    render(&mut renderer, &composition, 0.0, &media);
    renderer
        .precompile_frame_window(&composition, 1, 30, &media)
        .unwrap();
    let programs = renderer.created_program_count();
    if plans {
        assert_eq!(renderer.cached_plan_count(), PLANS as usize);
        assert_eq!(renderer.plan_compile_count(), u64::from(PLANS));
    }

    let mut plan_outputs = HashMap::new();
    for frame in 0..4 * PLANS {
        let (rendered, output) = render_held(&mut renderer, &composition, f64::from(frame), &media);
        assert_eq!(
            renderer.created_program_count(),
            programs,
            "frame {frame} built a program"
        );
        if plans {
            let first = plan_outputs
                .entry(frame % PLANS)
                .or_insert_with(|| Arc::clone(&output));
            assert!(
                Arc::ptr_eq(first, &output),
                "frame {frame} recompiled a plan the lookahead had already built"
            );
            assert_eq!(
                renderer.plan_compile_count(),
                u64::from(PLANS),
                "frame {frame}"
            );
        }
        assert_uniform(
            &rendered.rgba,
            plan_color(frame % PLANS),
            &format!("frame {frame}"),
        );

        renderer
            .precompile_frame_window(&composition, frame + 1, 30, &media)
            .unwrap();
        let size = lumen_gpu::Size::new(rendered.width, rendered.height);
        let after = read_rgba8(renderer.gpu_renderer(), &output, size);
        assert_eq!(
            after, rendered.rgba,
            "frame {frame} changed under the lookahead"
        );
    }
}

#[test]
fn lookahead_leaves_a_bound_frame_on_its_own_plan() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let shader =
        |color: &str| Composition::from(shader_scene(&fill_shader(color), json!({}), &[[0; 4]]));
    let red = shader("vec4<f32>(1.0, 0.0, 0.0, 1.0)");
    let green = shader("vec4<f32>(0.0, 1.0, 0.0, 1.0)");

    let bound = renderer.bind_frame(&red, 0, &media).unwrap();
    renderer
        .precompile_frame_window(&green, 0, 4, &media)
        .unwrap();
    let (raster, _) = renderer.submit_bound_frame(&bound).unwrap();
    let gpu_renderer = renderer.gpu_renderer();
    let texture = gpu_renderer.texture(raster.texture).unwrap();
    let pixels = read_rgba8(gpu_renderer, texture, raster.domain.storage_size);
    assert_uniform(
        &pixels,
        [255, 0, 0, 255],
        "the frame bound before the lookahead",
    );

    let next = render(&mut renderer, &green, 0.0, &media);
    assert_uniform(&next.rgba, [0, 255, 0, 255], "the next bind");
}

#[test]
fn lookahead_compiles_a_rejected_plan_once() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition = Composition::from(switch_scene(9));
    render(&mut renderer, &composition, 0.0, &media);
    renderer.set_plan_cache_budget(renderer.plan_cache_bytes());

    let compiles = renderer.plan_compile_count();
    let (programs, bytes) = (
        renderer.created_program_count(),
        renderer.plan_cache_bytes(),
    );
    for _ in 0..6 {
        renderer
            .precompile_frame_window(&composition, 1, 30, &media)
            .unwrap();
    }
    assert_eq!(renderer.created_program_count(), programs);
    // Recording has nothing to build ahead, so it holds no more for looking ahead.
    if renderer.frame_graph_enabled() {
        assert_eq!(renderer.plan_cache_bytes(), bytes);
        return;
    }
    assert_eq!(renderer.cached_plan_count(), 1);
    assert_eq!(renderer.plan_compile_count(), compiles + 1);
}

#[test]
fn rejected_plan_memory_is_bounded_and_released() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition = Composition::from(animated_shader_scene(9));
    render(&mut renderer, &composition, 0.0, &media);
    renderer.set_plan_cache_budget(0);
    // Recording rejects no plans. Its limit is that a frame's textures are all it keeps with no
    // budget, however many shader combinations go by, and that a release frees them.
    if renderer.frame_graph_enabled() {
        let bytes = renderer.plan_cache_bytes();
        for frame in 1..512_u32 {
            let rendered = render(&mut renderer, &composition, f64::from(frame), &media);
            assert_uniform(
                &rendered.rgba,
                [frame.min(255) as u8, 0, 0, 255],
                &format!("frame {frame}"),
            );
            assert!(renderer.plan_cache_bytes() <= bytes, "frame {frame}");
        }
        renderer.release_resources();
        assert_eq!(renderer.plan_cache_bytes(), 0);
        return;
    }
    for frame in 1..512 {
        renderer
            .precompile_frame(&composition, frame, &media)
            .unwrap();
    }
    assert!(renderer.rejected_plan_count() > 0);
    assert!(renderer.rejected_plan_count() <= 256);

    renderer.release_resources();
    assert_eq!(renderer.rejected_plan_count(), 0);
}

#[test]
fn shrinking_the_canvas_forgets_rejected_plan_estimates() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let with_canvas = |width: u32, height: u32| {
        let mut scene = switch_scene(9);
        scene.0["render_settings"] = json!({ "width": width, "height": height });
        Composition::from(scene)
    };
    let large = with_canvas(128, 128);
    render(&mut renderer, &large, 0.0, &media);
    let plan_bytes = renderer.plan_cache_bytes();
    // Recording keeps no estimates. With no budget its large textures go once the small frame runs.
    if renderer.frame_graph_enabled() {
        renderer.set_plan_cache_budget(0);
        let small = with_canvas(8, 4);
        let rendered = render(&mut renderer, &small, 1.0, &media);
        assert_uniform(&rendered.rgba, plan_color(1), "the small canvas");
        assert!(renderer.plan_cache_bytes() < plan_bytes);
        return;
    }
    renderer.set_plan_cache_budget(plan_bytes + plan_bytes / 4);
    renderer.precompile_frame(&large, 1, &media).unwrap();
    assert_eq!(renderer.cached_plan_count(), 1);
    assert_eq!(renderer.rejected_plan_count(), 1);

    renderer
        .precompile_frame(&with_canvas(8, 4), 1, &media)
        .unwrap();
    assert_eq!(renderer.cached_plan_count(), 2);
    assert_eq!(renderer.rejected_plan_count(), 0);
}

#[test]
fn the_plan_budget_bounds_independently_animated_shaders() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition = Composition::from(animated_shader_scene(6));

    renderer.compile(&composition).unwrap();
    // A recorded frame holds nothing until it renders; one frame's textures stand in for a plan.
    if renderer.frame_graph_enabled() {
        render(&mut renderer, &composition, 0.0, &media);
    }
    let plan_bytes = renderer.plan_cache_bytes();
    assert!(plan_bytes > 0);
    // The second budget cannot fit even one lookahead plan beside the current one.
    for (budget, max_plans) in [(3 * plan_bytes, 3), (plan_bytes / 2, 2)] {
        renderer.compile(&composition).unwrap();
        renderer.set_plan_cache_budget(budget);
        for frame in 0..72 {
            let (rendered, output) =
                render_held(&mut renderer, &composition, f64::from(frame), &media);
            renderer
                .precompile_frame_window(&composition, frame + 1, 30, &media)
                .unwrap();
            let (_, current) = render_held(&mut renderer, &composition, f64::from(frame), &media);
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
            let size = lumen_gpu::Size::new(rendered.width, rendered.height);
            let after = read_rgba8(renderer.gpu_renderer(), &output, size);
            assert_uniform(
                &after,
                [(frame % 64) as u8, 0, 0, 255],
                &format!("budget {budget}, frame {frame}"),
            );
        }
    }
}

#[test]
fn standalone_compile_keeps_animated_sources_full_frame_for_fractional_binds() {
    let composition = Composition::from(
        scene(
            (64, 16),
            json!([{ "id": 1, "type": "shape", "params": {
                "position": "=vec2(20 + 30 * sin(frame * 3.141592653589793), 4)",
                "width": 8, "height": 8, "fill_paint": [255, 100, 0, 255]
            } }]),
        )
        .frames(2)
        .fps(24.0),
    );
    let shape = NodeId::new(1);
    let shape_domain = |compiled: &CompiledComposition| {
        compiled
            .node_output(shape, &[])
            .expect("the shape compiled")
            .domain
    };

    let standalone = CompileContext::new(&composition).compile().unwrap();
    assert!(shape_domain(&standalone).is_full_frame());

    let gpu = gpu();
    let mut renderer = gpu.renderer();
    // A recorded frame bounds what each bind draws, including fractional ones.
    if renderer.frame_graph_enabled() {
        let media = TestMedia::default();
        for frame in [0.0, 0.25, 0.5, 1.5] {
            let recorded = render(&mut renderer, &composition, frame, &media);
            let reference = render(&mut gpu.reference(), &composition, frame, &media);
            assert!(recorded.rgba == reference.rgba, "frame {frame}");
            assert!(
                !shape_is_full_frame(&renderer, &composition),
                "frame {frame}"
            );
        }
        return;
    }
    renderer.compile(&composition).unwrap();
    assert!(!shape_domain(renderer.compiled().unwrap()).is_full_frame());
}

/// A static layer (nodes 1 to 3: a background and a square) under a square moving with the frame.
fn static_under_animated() -> Scene {
    scene(
        (16, 8),
        json!([
            { "id": 1, "type": "background", "params": { "paint": [200, 30, 30, 255], "width": 16, "height": 8 } },
            { "id": 2, "type": "shape", "params": {
                "position": [4, 2], "width": 4, "height": 4, "fill_paint": [20, 40, 220, 255]
            } },
            { "id": 3, "type": "raster_multimerge", "in": { "layers": [1, 2] } },
            { "id": 4, "type": "shape", "params": {
                "position": "=vec2(frame * 2, 3)", "width": 3, "height": 3, "fill_paint": [30, 220, 60, 255]
            } },
            { "id": 5, "type": "raster_multimerge", "in": { "layers": [3, 4] } },
        ]),
    )
    .frames(4)
}

#[test]
fn static_output_reuse_skips_passes_and_changes_no_pixel() {
    let gpu = gpu();
    let media = TestMedia::default();
    let composition = Composition::from(static_under_animated());
    for reuse in [true, false] {
        let mut renderer = gpu.renderer();
        renderer.set_static_output_reuse(reuse);
        let mut reference = gpu.reference();
        let mut skipped = Vec::new();
        let mut frames = Vec::new();
        for frame in 0..4 {
            let at = f64::from(frame);
            let actual = render(&mut renderer, &composition, at, &media);
            let expected = render(&mut reference, &composition, at, &media);
            if let Err(error) = within_rounding(&actual.rgba, &expected.rgba) {
                panic!("reuse {reuse}, frame {frame} differs from an uncached render: {error}");
            }
            skipped.push(renderer.cached_pass_count());
            frames.push(actual.rgba);
        }
        assert!(frames[0] != frames[3], "the animated square still moves");
        if reuse {
            let per_frame = skipped[1];
            assert!(
                per_frame >= 3,
                "the background, square and their merge are kept"
            );
            assert_eq!(skipped, [0, per_frame, 2 * per_frame, 3 * per_frame]);
        } else {
            assert_eq!(skipped, [0; 4]);
        }
    }
}

fn shape_is_full_frame(
    renderer: &lumen_engine::gpu::GpuCompositionRenderer,
    composition: &Composition,
) -> bool {
    renderer
        .recorded_domain(composition, NodeId::new(1))
        .expect("the shape recorded")
        .is_full_frame()
}
