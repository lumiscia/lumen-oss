//! Output kept between frames: what `memo` holds, what static layers keep, and how both survive
//! plan switches, edits, external targets and the shared text atlas.

use lumen_engine::{composition::Composition, gpu::GpuCompositionRenderer, node::NodeId};
use lumen_gpu::wgpu;
use serde_json::{Value, json};

use crate::support::{Scene, TestMedia, gpu, read_rgba8, render, scene};

const CANVAS: (u32, u32) = (32, 16);
const RED: [u8; 4] = [255, 0, 0, 255];

const MEMO_FRAME_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = vec4<f32>(f32(lumen.frame) / 255.0, lumen.time, 0.5, 1.0);
    textureStore(output, vec2<i32>(id.xy), color);
}
";

fn canvas(nodes: Vec<Value>) -> Scene {
    scene(CANVAS, json!(nodes)).frames(12)
}

fn background(id: u32, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "background", "params": {
        "paint": color, "width": CANVAS.0, "height": CANVAS.1
    } })
}

fn square(id: u32, position: Value, size: u32, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "shape", "params": {
        "position": position, "width": size, "height": size, "fill_paint": color
    } })
}

fn merge(id: u32, layers: &[u32]) -> Value {
    json!({ "id": id, "type": "raster_multimerge", "in": { "layers": layers } })
}

fn memo(id: u32, source: u32, key: &str) -> Value {
    json!({ "id": id, "type": "memo", "params": { "key": format!("={key}") }, "in": { "source": source } })
}

fn switch(id: u32, selected_layer: &str, layers: &[u32]) -> Value {
    json!({ "id": id, "type": "switch", "params": { "selected_layer": format!("={selected_layer}") },
        "in": { "layers": layers } })
}

/// A static layer (node 3) under a square moving with the frame.
fn static_under_animated_nodes() -> Vec<Value> {
    vec![
        background(1, [200, 30, 30, 255]),
        square(2, json!([4, 4]), 8, [20, 40, 220, 255]),
        merge(3, &[1, 2]),
        square(4, json!("=vec2(frame * 2, 6)"), 4, [30, 220, 60, 255]),
        merge(5, &[3, 4]),
    ]
}

fn static_under_animated() -> Scene {
    canvas(static_under_animated_nodes())
}

/// A shader whose colour follows the frame, behind a memo keyed by `key` when there is one.
fn memo_shader(key: Option<&str>) -> Scene {
    let mut nodes = vec![
        background(4, [0, 0, 0, 255]),
        json!({ "id": 1, "type": "wgsl_shader", "params": { "shader": MEMO_FRAME_SHADER },
            "in": { "source": 4 } }),
    ];
    nodes.extend(key.map(|key| memo(2, 1, key)));
    canvas(nodes)
}

fn at(renderer: &mut GpuCompositionRenderer, composition: &Composition, frame: f64) -> Vec<u8> {
    render(renderer, composition, frame, &TestMedia::default()).rgba
}

/// Renders `frame` without any output kept from earlier frames.
fn uncached(
    renderer: &mut GpuCompositionRenderer,
    composition: &Composition,
    frame: f64,
) -> Vec<u8> {
    renderer.release_resources();
    at(renderer, composition, frame)
}

fn read_raster(
    renderer: &GpuCompositionRenderer,
    raster: lumen_engine::gpu::RasterHandle,
) -> Vec<u8> {
    let gpu = renderer.gpu_renderer();
    read_rgba8(
        gpu,
        gpu.texture(raster.texture).unwrap(),
        raster.domain.storage_size,
    )
}

fn nonblank(pixels: &[u8]) -> bool {
    pixels.iter().any(|&byte| byte > 0)
}

#[test]
fn memo_renders_its_source_only_when_the_key_changes() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let memoized: Composition = memo_shader(Some("floor(frame / 4)")).into();
    let plain: Composition = memo_shader(None).into();
    let mut skipped = Vec::new();
    let mut frames = Vec::new();
    for frame in 0..9 {
        let before = renderer.cached_pass_count();
        let pixels = at(&mut renderer, &memoized, f64::from(frame));
        skipped.push(renderer.cached_pass_count() - before);
        let shown = f64::from(frame - frame % 4);
        assert!(
            pixels == uncached(&mut reference, &plain, shown),
            "frame {frame} shows the shader at frame {shown}"
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
    assert!(frames[3] != frames[4], "a refresh shows the new frame");
}

#[test]
fn plan_cache_budget_keeps_the_plan_a_bound_frame_copies_from() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let composition: Composition = memo_shader(Some("frame * 0")).into();
    let first = render(&mut renderer, &composition, 0.0, &media).rgba;
    assert!(nonblank(&first));
    let bound = renderer.bind_frame_at(&composition, 0.5, &media).unwrap();
    renderer.set_plan_cache_budget(0);
    let (raster, _) = renderer.submit_bound_frame(&bound).unwrap();
    assert!(read_raster(&renderer, raster) == first);
}

#[test]
fn memo_key_is_evaluated_at_fractional_frame_times() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let memoized: Composition = memo_shader(Some("floor(frame * 2)")).into();
    let plain: Composition = memo_shader(None).into();
    // The key changes at 0.5, inside the whole frame 0, which a key read at floor(frame) would
    // miss. Refreshes fall between key boundaries, so each must show its own time.
    for (time, shown) in [
        (0.25, 0.25),
        (0.4, 0.25),
        (0.75, 0.75),
        (0.9, 0.75),
        (1.25, 1.25),
    ] {
        let pixels = at(&mut renderer, &memoized, time);
        let expected = uncached(&mut reference, &plain, shown);
        assert!(nonblank(&expected));
        assert!(
            pixels == expected,
            "frame {time} shows the shader at {shown}"
        );
        if time != shown {
            assert!(
                uncached(&mut reference, &plain, time) != expected,
                "the shader animates between {shown} and {time}"
            );
        }
    }
}

/// One held-snapshot row: the scene for a memo `key`, and the frames that must all show the
/// first one.
struct Hold {
    name: &'static str,
    scene: fn(&str) -> Scene,
    frames: &'static [f64],
    plans: usize,
}

fn cropped_clip(key: &str) -> Scene {
    canvas_4x4(vec![
        background_4x4(1),
        json!({ "id": 2, "type": "crop", "params": { "width": 2, "height": 2 }, "in": { "source": 1 } }),
        json!({ "id": 3, "type": "clip", "params": { "duration": 2 }, "in": { "source": 2 } }),
        memo(4, 3, key),
    ])
}

fn cropped_template_clip(key: &str) -> Scene {
    let mut scene = canvas_4x4(vec![
        json!({ "id": 3, "type": "clip", "params": { "composition": "cropped", "duration": 2 } }),
        memo(4, 3, key),
    ]);
    scene.0["compositions"] = json!({ "cropped": {
        "timeline": { "fps": 10, "duration_frames": 10 },
        "render_settings": { "width": 4, "height": 4 },
        "nodes": [
            background_4x4(1),
            { "id": 2, "type": "crop", "params": { "width": 2, "height": 2 } },
            { "id": 5, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 2, "to_port": "source" },
            { "from_node": 2, "to_node": 5, "to_port": "source" }
        ]
    } });
    scene
}

/// A switch between an uncropped layer and a cropped one, behind a memo.
fn cropped_switch(key: &str) -> Scene {
    canvas_4x4(vec![
        background_4x4(1),
        json!({ "id": 2, "type": "crop", "params": { "width": 2, "height": 2 }, "in": { "source": 1 } }),
        switch(3, "floor(frame / 2) % 2", &[1, 2]),
        memo(4, 3, key),
    ])
}

fn canvas_4x4(nodes: Vec<Value>) -> Scene {
    scene((4, 4), json!(nodes)).frames(10)
}

fn background_4x4(id: u32) -> Value {
    json!({ "id": id, "type": "background", "params": { "paint": RED, "width": 4, "height": 4 } })
}

/// A crop moving with the frame over a static layer, behind a memo, optionally read by a resize.
fn moving_crop(key: &str, resize: bool) -> Scene {
    let mut nodes = vec![
        background(1, RED),
        square(2, json!([6, 6]), 6, [20, 40, 220, 255]),
        merge(3, &[1, 2]),
        json!({ "id": 4, "type": "crop", "params": { "x": "=frame * 4", "width": 8, "height": 8 },
            "in": { "source": 3 } }),
        memo(5, 4, key),
    ];
    if resize {
        nodes.push(json!({ "id": 6, "type": "resize", "params": {
            "width": CANVAS.0, "height": CANVAS.1, "mode": "fit", "sampling": "nearest"
        }, "in": { "source": 5 } }));
    }
    canvas(nodes)
}

fn switched_shader(key: &str) -> Scene {
    canvas(vec![
        background(4, [0, 0, 0, 255]),
        json!({ "id": 1, "type": "wgsl_shader", "params": { "shader": MEMO_FRAME_SHADER },
            "in": { "source": 4 } }),
        background(5, [0, 255, 0, 255]),
        switch(6, "floor(frame / 2) % 2", &[1, 5]),
        memo(2, 6, key),
    ])
}

#[test]
fn memo_snapshot_is_adopted_by_the_plans_its_key_spans() {
    let rows = [
        Hold {
            name: "shader, whole and fractional frames",
            scene: |key| memo_shader(Some(key)),
            frames: &[0.0, 0.5, 1.0],
            plans: 2,
        },
        Hold {
            name: "square sized to its bounds over whole frames",
            scene: |key| {
                canvas(vec![
                    square(1, json!("=vec2(frame * 4, 4)"), 4, RED),
                    memo(2, 1, key),
                ])
            },
            frames: &[0.0, 0.5, 1.0],
            plans: 2,
        },
        Hold {
            name: "switch upstream selects another plan every two frames",
            scene: switched_shader,
            frames: &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0],
            plans: 2,
        },
        Hold {
            name: "crop bounds read directly",
            scene: |key| moving_crop(key, false),
            frames: &[0.0, 0.5, 1.0],
            plans: 2,
        },
        Hold {
            name: "crop bounds read by a resize",
            scene: |key| moving_crop(key, true),
            frames: &[0.0, 0.5, 1.0],
            plans: 2,
        },
        Hold {
            name: "cropped source behind a clip that hides",
            scene: cropped_clip,
            frames: &[0.0, 2.0],
            plans: 2,
        },
        Hold {
            name: "cropped template behind a clip that hides",
            scene: cropped_template_clip,
            frames: &[0.0, 2.0],
            plans: 2,
        },
        Hold {
            name: "switch between uncropped and cropped layers",
            scene: cropped_switch,
            frames: &[0.0, 2.0, 4.0, 6.0],
            plans: 2,
        },
    ];
    let gpu = gpu();
    for row in rows {
        let held: Composition = (row.scene)("floor(frame / 100)").into();
        let refreshed: Composition = (row.scene)("frame").into();
        let mut renderer = gpu.renderer();
        let first = at(&mut renderer, &held, row.frames[0]);
        assert!(nonblank(&first), "{}: the first frame is blank", row.name);
        for &frame in &row.frames[1..] {
            assert!(
                at(&mut renderer, &held, frame) == first,
                "{}: frame {frame} does not show the first snapshot",
                row.name
            );
        }
        let last = *row.frames.last().unwrap();
        assert!(
            at(&mut gpu.renderer(), &refreshed, last) != first,
            "{}: a memo refreshed every frame shows the same pixels",
            row.name
        );
        assert!(
            renderer.cached_plan_count() >= row.plans,
            "{}: only {} plans were compiled",
            row.name,
            renderer.cached_plan_count()
        );
    }
}

#[test]
fn memo_snapshots_do_not_survive_composition_edits() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let build = |width: u32| -> Composition {
        canvas(vec![
            json!({ "id": 99, "type": "shape", "params": {
                "position": [0, 0], "width": width, "height": 4, "fill_paint": [0, 255, 0, 255]
            } }),
            square(1, json!("=vec2(node(99, 'width'), 0)"), 4, RED),
            memo(2, 1, "0 + frame * 0"),
        ])
        .into()
    };
    let before = at(&mut renderer, &build(4), 0.0);
    let edited = build(16);
    let expected = at(&mut gpu.renderer(), &edited, 0.0);
    assert!(expected != before, "the edit moves the square");
    // The existing plan, then a plan compiled for a fractional frame.
    for frame in [1.0, 0.5] {
        assert!(
            at(&mut renderer, &edited, frame) == expected,
            "frame {frame} shows the edited composition"
        );
    }
}

#[test]
fn memo_snapshot_dropped_with_its_key_change_is_not_shown_again() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    // Frame 1 places more glyphs than the atlas holds, so its frame is not kept.
    let composition: Composition = canvas(vec![
        background(4, [0, 0, 0, 255]),
        json!({ "id": 1, "type": "wgsl_shader", "params": { "shader": MEMO_FRAME_SHADER },
            "in": { "source": 4 } }),
        memo(2, 1, "floor(frame) % 2"),
        json!({ "id": 5, "type": "text", "params": {
            "content": "=if(floor(frame) % 2 == 1, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'W')",
            "font_size": 1500, "scale": [0.001, 0.001], "position": [30, 10],
            "render_mode": "raster"
        } }),
        merge(6, &[2, 5]),
    ])
    .into();
    let plain: Composition = memo_shader(None).into();
    // Frames 0 and 0.5 are bound to different plans, each holding key 0; frame 2 has key 0 again.
    let first_pixel = |pixels: &[u8]| pixels[..4].to_vec();
    let at_zero = first_pixel(&uncached(&mut reference, &plain, 0.0));
    let at_two = first_pixel(&uncached(&mut reference, &plain, 2.0));
    assert!(
        at_zero != at_two,
        "the shader differs between frames 0 and 2"
    );
    for frame in [0.0, 0.5, 1.0, 2.0] {
        let expected = first_pixel(&uncached(&mut reference, &plain, f64::floor(frame)));
        assert!(
            first_pixel(&at(&mut renderer, &composition, frame)) == expected,
            "frame {frame} shows the shader at frame {}",
            f64::floor(frame)
        );
    }
}

/// A `tile` template of two canvas-wide halves, whose node ids repeat the root's, and one or two
/// clips of it, the second moved to the right half.
fn tile_clips(clips: Value, template_nodes: Value) -> Composition {
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
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let mut kept_per_frame = |composition: &Composition| {
        let mut cached = Vec::new();
        let mut frames = Vec::new();
        for frame in 2..6 {
            let pixels = at(&mut renderer, composition, f64::from(frame));
            assert!(
                pixels == uncached(&mut reference, composition, f64::from(frame)),
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
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    // Each clip shows its local frame rounded down to a multiple of 4.
    let held = |local: u32| (local - local % 4) as u8 * 10;
    let mut skipped = 0;
    for frame in 0..12 {
        let before = renderer.cached_pass_count();
        let pixels = at(&mut renderer, &composition, f64::from(frame));
        skipped += renderer.cached_pass_count() - before;
        assert_eq!(pixels[0], held(frame), "first clip at frame {frame}");
        if frame >= 2 {
            assert_eq!(pixels[8], held(frame - 2), "second clip at frame {frame}");
        }
    }
    assert!(skipped > 0, "held memos skip their sources");
}

/// Static layers 4 and 5 sharing square 3, and 7 with a third colour, selected in turn every two
/// frames, under a moving square.
fn switching_static_layers() -> Composition {
    canvas(vec![
        background(1, [200, 30, 30, 255]),
        background(2, [30, 200, 30, 255]),
        background(8, [30, 30, 200, 255]),
        square(3, json!([4, 4]), 8, [255, 255, 255, 255]),
        merge(4, &[1, 3]),
        merge(5, &[2, 3]),
        merge(9, &[8, 3]),
        switch(6, "floor(frame / 2) % 3", &[4, 5, 9]),
        square(7, json!("=vec2(frame * 2, 6)"), 4, [20, 40, 220, 255]),
        merge(10, &[6, 7]),
    ])
    .into()
}

#[test]
fn kept_output_survives_plan_switches_and_is_dropped_with_evicted_plans() {
    let gpu = gpu();
    let mut reference = gpu.renderer();
    let composition = switching_static_layers();
    // Each of the three selections is its own plan; frames 6 and 7 select the first again.
    for budget in [None, Some(0)] {
        let mut renderer = gpu.renderer();
        if let Some(budget) = budget {
            renderer.set_plan_cache_budget(budget);
        }
        let mut compiled_before_revisit = 0;
        for frame in 0..8 {
            if frame == 6 {
                compiled_before_revisit = renderer.plan_compile_count();
            }
            let pixels = at(&mut renderer, &composition, f64::from(frame));
            let expected = uncached(&mut reference, &composition, f64::from(frame));
            assert!(pixels == expected, "frame {frame} with budget {budget:?}");
            assert!(nonblank(&pixels));
        }
        assert!(renderer.cached_pass_count() > 0, "budget {budget:?}");
        assert_eq!(
            renderer.plan_compile_count() > compiled_before_revisit,
            budget.is_some(),
            "revisiting the first selection with budget {budget:?}"
        );
        let kept = renderer.cached_plan_count();
        assert!(
            if budget.is_some() {
                kept <= 2
            } else {
                kept == 3
            },
            "{kept} plans kept with budget {budget:?}"
        );
    }
}

#[test]
fn replacing_a_kept_texture_renders_it_again() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let composition: Composition = static_under_animated().into();
    for frame in 0..2 {
        at(&mut renderer, &composition, f64::from(frame));
    }
    assert!(renderer.cached_pass_count() > 0, "the static layer is kept");
    let kept = renderer
        .compiled()
        .unwrap()
        .node_output(NodeId::new(3), &[])
        .unwrap();
    let gpu = renderer.gpu_renderer_mut();
    let desc = gpu.texture_desc(kept.texture).unwrap();
    let blank = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("blank"),
        size: desc.domain.storage_size.as_extent(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: desc.format,
        usage: desc.usage,
        view_formats: &[],
    });
    gpu.replace_texture(kept.texture, blank, desc).unwrap();
    let pixels = at(&mut renderer, &composition, 2.0);
    let expected = uncached(&mut reference, &composition, 2.0);
    assert!(nonblank(&expected));
    assert!(pixels == expected, "the replaced texture is rendered again");
}

#[test]
fn discarded_binds_keep_nothing_and_stale_uploads_are_still_rejected() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let mut composition = static_under_animated();
    composition.0["nodes"][1] = json!({ "id": 2, "type": "text", "params": {
        "content": "Kept", "font_size": 12, "position": [2, 8]
    } });
    let composition: Composition = composition.into();
    let media = TestMedia::default();
    let submit = |renderer: &mut GpuCompositionRenderer, frame| {
        let bound = renderer.bind_frame(&composition, frame, &media).unwrap();
        let (raster, _) = renderer.submit_bound_frame(&bound).unwrap();
        read_raster(renderer, raster)
    };
    for (discarded, submitted) in [(0, 1), (2, 3)] {
        renderer
            .bind_frame(&composition, discarded, &media)
            .unwrap();
        let pixels = submit(&mut renderer, submitted);
        assert!(
            pixels == uncached(&mut reference, &composition, f64::from(submitted)),
            "frame {submitted} after discarding a bind of frame {discarded}"
        );
        assert!(nonblank(&pixels));
    }
    assert!(renderer.cached_pass_count() > 0);

    let older = renderer.bind_frame(&composition, 4, &media).unwrap();
    let current = renderer.bind_frame(&composition, 5, &media).unwrap();
    assert!(renderer.upload_bound_frame(&older).is_err());
    renderer.upload_bound_frame(&current).unwrap();
    let (raster, _) = renderer.submit_render().unwrap();
    assert!(read_raster(&renderer, raster) == uncached(&mut reference, &composition, 5.0));
}

#[test]
fn kept_output_reaches_external_output_textures() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let fully_static: Composition = canvas(vec![
        background(1, [200, 30, 30, 255]),
        square(2, json!([4, 4]), 8, [20, 40, 220, 255]),
        merge(3, &[1, 2]),
    ])
    .into();
    // Only a memo holds the whole animated output.
    let held_root: Composition = canvas({
        let mut nodes = static_under_animated_nodes();
        nodes.push(memo(7, 5, "floor(frame / 2)"));
        nodes
    })
    .into();
    let plain: Composition = static_under_animated().into();
    for (composition, expected_from, held) in [
        (&fully_static, &fully_static, false),
        (&held_root, &plain, true),
    ] {
        renderer.release_resources();
        at(&mut renderer, composition, 0.0);
        let output = renderer.compiled().unwrap().output;
        let desc = renderer
            .gpu_renderer()
            .texture_desc(output.texture)
            .unwrap();
        let mut shown = Vec::new();
        for frame in 1..4 {
            let texture = renderer
                .gpu_renderer()
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("external output"),
                    size: desc.domain.storage_size.as_extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: desc.format,
                    usage: desc.usage | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
            let submitted = renderer
                .render_frame_into_external(
                    composition,
                    frame,
                    &TestMedia::default(),
                    lumen_gpu::ExternalTexture::from_texture(texture, desc.domain),
                )
                .unwrap();
            submitted.wait(&renderer.gpu_renderer().device).unwrap();
            let pixels = read_rgba8(
                renderer.gpu_renderer(),
                submitted.texture(),
                desc.domain.storage_size,
            );
            let source_frame = if held { frame - frame % 2 } else { frame };
            assert!(
                pixels == uncached(&mut reference, expected_from, f64::from(source_frame)),
                "external frame {frame} (held root: {held})"
            );
            shown.push(pixels);
        }
        assert!(renderer.cached_pass_count() > 0);
        assert!(nonblank(&shown[0]));
        assert_eq!(
            [shown[0] != shown[1], shown[1] == shown[2]],
            [held, true],
            "the output changes only at a refresh, if at all (held root: {held})"
        );
    }
}

#[test]
fn kept_text_keeps_its_glyphs_in_the_shared_atlas() {
    let gpu = gpu();
    let (mut renderer, mut reference) = (gpu.renderer(), gpu.renderer());
    let text = |id: u32, content: &str, font_size: Value, x: u32| {
        json!({ "id": id, "type": "text", "params": {
            "content": content, "font_size": font_size, "scale": [0.03, 0.03],
            "position": [x, 0], "render_mode": "raster"
        } })
    };
    // Glyphs this large fill most of the atlas, so the static "W" and the "M" that grows on
    // frame 1 cannot both fit; the "W" is drawn first, so it keeps its glyph.
    let composition: Composition = scene(
        (256, 128),
        json!([
            text(1, "W", json!(3800), 0),
            text(2, "M", json!("=12 + 3788 * min(frame, 1)"), 120),
            merge(3, &[1, 2]),
        ]),
    )
    .frames(3)
    .into();
    for frame in 0..3 {
        let pixels = at(&mut renderer, &composition, f64::from(frame));
        assert!(nonblank(&pixels));
        assert!(
            pixels == uncached(&mut reference, &composition, f64::from(frame)),
            "frame {frame} differs from an uncached render"
        );
    }
    assert!(renderer.cached_pass_count() > 0, "the static text is kept");
}
