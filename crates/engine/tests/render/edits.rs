//! How one renderer follows a composition as it is edited, what it keeps from frame to frame, and
//! what it lets go of.

use std::sync::Arc;

use lumen_engine::{composition::Composition, node::NodeId};
use serde_json::{Value, json};

use crate::color::srgb8;
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

fn layer_color(index: u32) -> [u8; 4] {
    [index as u8 * 25 + 5, 0, 0, 255]
}

/// An 8 × 4 scene that shows `layers` solid layers in turn.
fn switch_scene(layers: u32) -> Scene {
    let mut nodes: Vec<Value> = (0..layers)
        .map(|index| {
            json!({ "id": index + 1, "type": "background", "params": {
                "paint": layer_color(index), "width": 8, "height": 4
            } })
        })
        .collect();
    nodes.push(json!({
        "id": 100, "type": "switch", "in": { "layers": (1..=layers).collect::<Vec<_>>() },
        "params": { "selected_layer": format!("=frame % {layers}") }
    }));
    scene((8, 4), json!(nodes)).frames(120)
}

/// An 8 × 4 scene of `bits` shaders in a row, shader `bit` adding 2^bit / 256 to the light in red
/// on alternating runs of 2^bit frames, so it has 2^bits distinct combinations of shaders. Sums of
/// those steps are exact in half floats.
fn animated_shader_scene(bits: u32) -> Scene {
    let add_red = |red: u32| {
        format!(
            "@compute @workgroup_size(8, 8, 1) fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) \
             {{ let color = textureLoad(input0, vec2<i32>(id.xy), 0); \
             textureStore(output, vec2<i32>(id.xy), color + vec4<f32>({red}.0 / 256.0, 0.0, 0.0, 0.0)); }}"
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

/// A shader filling with the sRGB red code `red`.
fn red_shader(red: u8) -> Composition {
    let color =
        format!("vec4<f32>(lumen_srgb_to_linear(vec3<f32>({red}.0 / 255.0, 0.0, 0.0)), 1.0)");
    Composition::from(shader_scene(&fill_shader(&color), json!({}), &[[0; 4]]))
}

#[test]
fn shader_edits_keep_held_textures() {
    let gpu = gpu();
    let media = TestMedia::default();
    let mut renderer = gpu.renderer();
    let mut outputs = Vec::new();
    let mut held = None;
    for edit in 0..12u8 {
        let red = (edit + 1) * 20;
        let composition = red_shader(red);
        let (frame, texture) = render_held(&mut renderer, &composition, 0.0, &media);
        assert_uniform(&frame.rgba, [red, 0, 0, 255], &format!("edit {edit}"));
        held.get_or_insert_with(|| (texture.as_ref().clone(), frame.width, frame.height));
        outputs.push(Arc::downgrade(&texture));
    }
    let resident = outputs
        .iter()
        .filter(|output| output.strong_count() > 0)
        .count();
    assert_eq!(resident, 1, "superseded outputs are still resident");

    let (texture, width, height) = held.unwrap();
    let size = lumen_gpu::Size::new(width, height);
    let pixels = read_rgba8(renderer.gpu_renderer(), &texture, size);
    assert_uniform(
        &pixels,
        [20, 0, 0, 255],
        "the texture held from the first edit",
    );
}

#[test]
fn switching_layers_builds_no_program_after_the_first_frame() {
    const LAYERS: u32 = 9;
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition = Composition::from(switch_scene(LAYERS));

    render(&mut renderer, &composition, 0.0, &media);
    let programs = renderer.created_program_count();
    for frame in 0..4 * LAYERS {
        let rendered = render(&mut renderer, &composition, f64::from(frame), &media);
        assert_eq!(
            renderer.created_program_count(),
            programs,
            "frame {frame} built a program"
        );
        assert_uniform(
            &rendered.rgba,
            layer_color(frame % LAYERS),
            &format!("frame {frame}"),
        );
    }
}

/// With no budget a frame's textures are all the renderer keeps, however many shader combinations
/// go by, and a release frees them.
#[test]
fn with_no_budget_only_a_frames_textures_are_held() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition = Composition::from(animated_shader_scene(9));
    render(&mut renderer, &composition, 0.0, &media);
    renderer.set_cache_budgets(lumen_engine::gpu::CacheBudgets {
        pooled_textures: 0,
        ..renderer.cache_budgets()
    });
    let bytes = renderer.held_bytes();
    for frame in 1..512_u32 {
        let rendered = render(&mut renderer, &composition, f64::from(frame), &media);
        assert_uniform(
            &rendered.rgba,
            [srgb8(f64::from(frame) / 256.0), 0, 0, 255],
            &format!("frame {frame}"),
        );
        assert!(renderer.held_bytes() <= bytes, "frame {frame}");
    }
    renderer.release_resources();
    assert_eq!(renderer.held_bytes(), 0);
}

#[test]
fn with_no_budget_a_smaller_canvas_lets_the_larger_textures_go() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let with_canvas = |width: u32, height: u32| {
        let mut scene = switch_scene(9);
        scene.0["render_settings"] = json!({ "width": width, "height": height });
        Composition::from(scene)
    };
    render(&mut renderer, &with_canvas(128, 128), 0.0, &media);
    let large = renderer.held_bytes();
    renderer.set_cache_budgets(lumen_engine::gpu::CacheBudgets {
        pooled_textures: 0,
        ..renderer.cache_budgets()
    });
    let rendered = render(&mut renderer, &with_canvas(8, 4), 1.0, &media);
    assert_uniform(&rendered.rgba, layer_color(1), "the small canvas");
    assert!(renderer.held_bytes() < large);
}

/// A frame bounds what each bind draws, including binds between whole frames.
#[test]
fn an_animated_source_stays_bounded_at_fractional_binds() {
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
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    for frame in [0.0, 0.25, 0.5, 1.5] {
        let recorded = render(&mut renderer, &composition, frame, &media);
        let reference = render(&mut gpu.reference(), &composition, frame, &media);
        assert!(recorded.rgba == reference.rgba, "frame {frame}");
        let domain = renderer
            .recorded_domain(&composition, NodeId::new(1))
            .expect("the shape recorded");
        assert!(!domain.is_full_frame(), "frame {frame}");
    }
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
            skipped.push(renderer.skipped_pass_count());
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
