//! When a `clip` shows its content, and what canvas it places the content on.

use lumen_engine::{composition::Composition, media::collect_frame_requirements, node::NodeId};
use serde_json::{Value, json};

use crate::support::{Case, Expect, Scene, TestMedia, check, gpu, render, scene};

pub const CLEAR: [u8; 4] = [0, 0, 0, 0];

pub fn probe(red: u8) -> [u8; 4] {
    [red, 7, 0, 255]
}

/// A 1 × 1, 10 fps root that plays its `probe` template through the clip `clip`. The template
/// draws red at 10 × its frame and green at its `g` prop, which defaults to 7.
pub fn probe_scene(template_fps: f64, clip: Value) -> Scene {
    Scene(json!({
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
    }))
}

/// A row showing `colors[n].1` at frame `colors[n].0`.
fn timing(name: &str, scene: Scene, colors: &[(f64, [u8; 4])]) -> Case {
    let shown = colors.iter().map(|&(_, color)| Expect::Uniform(color));
    Case::new(name, scene, Expect::PerFrame(shown.collect()))
        .frames(colors.iter().map(|&(frame, _)| frame))
        .tolerance(0)
}

#[test]
fn clip_timing() {
    let window = json!({ "composition": "probe", "start": 2, "duration": 3 });
    let window_with = |end: &str| {
        let mut clip = window.clone();
        clip["end"] = json!(end);
        clip
    };
    let prop = |g: Value| json!({ "composition": "probe", "start": 2, "props": { "g": g } });
    let source_window = scene(
        (1, 1),
        json!([
            { "id": 1, "type": "background", "params": { "paint": "=rgb(frame * 10, 0, 0)" } },
            { "id": 2, "type": "clip", "in": { "source": 1 }, "params": {
                "start": 5, "duration": 4, "offset": 2
            } },
        ]),
    )
    .frames(20)
    .fps(10.0);
    let looped_remaps_in_nested_clips = scene(
        (1, 1),
        json!([
            { "id": 1, "type": "background", "params": { "paint": "#ff0000" } },
            { "id": 2, "type": "clip", "in": { "source": 1 }, "params": { "duration": 4 } },
            { "id": 3, "type": "clip", "in": { "source": 2 }, "params": { "duration": 2, "speed": 2 } },
            { "id": 4, "type": "time_remap", "in": { "source": 3 }, "params": {
                "frame": "=1 / 49 * 49", "loop_enabled": true, "loop_start": 1, "loop_end": 2
            } },
        ]),
    )
    .frames(30);
    check(vec![
        timing(
            "plays in local time inside its window and is hidden outside",
            probe_scene(10.0, window.clone()),
            &[
                (1.0, CLEAR),
                (2.0, probe(0)),
                (4.0, probe(20)),
                (5.0, CLEAR),
            ],
        ),
        timing(
            "hold keeps the last frame",
            probe_scene(10.0, window_with("hold")),
            &[
                (1.0, CLEAR),
                (4.0, probe(20)),
                (5.0, probe(20)),
                (30.0, probe(20)),
            ],
        ),
        timing(
            "loop plays the window again",
            probe_scene(10.0, window_with("loop")),
            &[
                (1.0, CLEAR),
                (4.0, probe(20)),
                (5.0, probe(0)),
                (9.0, probe(10)),
            ],
        ),
        // local = (frame - start) * speed * template fps / fps + offset, over the template's
        // remaining 8 frames
        timing(
            "speed and offset",
            probe_scene(
                10.0,
                json!({ "composition": "probe", "start": 2, "speed": 1.5, "offset": 2 }),
            ),
            &[
                (2.0, probe(20)),
                (3.0, probe(35)),
                (4.0, probe(50)),
                (7.0, probe(95)),
            ],
        ),
        // The template runs at twice the root's fps, so its 10 frames last 5 root frames.
        timing(
            "template at twice the root's fps",
            probe_scene(20.0, json!({ "composition": "probe", "start": 0 })),
            &[(3.0, probe(60)), (4.0, probe(80)), (5.0, CLEAR)],
        ),
        timing(
            "a literal prop",
            probe_scene(10.0, prop(json!(40))),
            &[(3.0, [10, 40, 0, 255])],
        ),
        // Evaluated at the root frame, so it differs from the template's own frame.
        timing(
            "a prop expression reads the parent's time",
            probe_scene(10.0, prop(json!("=frame * 10"))),
            &[(3.0, [10, 30, 0, 255]), (5.0, [30, 50, 0, 255])],
        ),
        timing(
            "a source clip is windowed like a time remap",
            source_window,
            &[
                (4.0, CLEAR),
                (5.0, [20, 0, 0, 255]),
                (8.0, [50, 0, 0, 255]),
                (9.0, CLEAR),
            ],
        ),
        timing(
            "a loop target does not round up to the hidden end of nested windows",
            looped_remaps_in_nested_clips,
            &[(0.0, [255, 0, 0, 255])],
        ),
    ]);
}

#[test]
fn clip_canvas() {
    let mut larger_than_parent = probe_scene(10.0, json!({ "composition": "probe", "start": 0 }));
    larger_than_parent.0["render_settings"] = json!({ "width": 3, "height": 2 });
    larger_than_parent.0["compositions"]["probe"]["render_settings"] =
        json!({ "width": 5, "height": 1 });
    let mut wider_than_template = scene(
        (16, 4),
        json!([
            { "id": 1, "type": "clip", "params": { "composition": "small" } },
        ]),
    );
    wider_than_template.0["compositions"] = json!({
        "small": {
            "render_settings": { "width": 8, "height": 4 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "width": 16, "height": 4, "paint": "#ff0000" } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
        }
    });
    check(vec![
        Case::new(
            "a template wider than its parent is cropped to the parent",
            larger_than_parent,
            Expect::Probes(vec![
                (0, 0, probe(20)),
                (2, 0, probe(20)),
                (0, 1, CLEAR),
                (2, 1, CLEAR),
            ]),
        )
        .frames([2.0])
        .tolerance(0),
        Case::new(
            "content past the template's canvas is cropped",
            wider_than_template,
            Expect::Probes(vec![
                (7, 0, [255, 0, 0, 255]),
                (8, 0, CLEAR),
                (15, 3, CLEAR),
            ]),
        )
        .tolerance(0),
    ]);
}

/// Three probe instances of one template: one in a strip played by the root, a second beside it
/// in the strip, and a third played by the root, each with its own props and time.
#[test]
fn clip_instances() {
    let scene = Scene(json!({
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
    }));
    let row = |colors: [[u8; 4]; 3]| {
        Expect::Probes(colors.iter().zip(0u32..).map(|(&c, x)| (x, 0, c)).collect())
    };
    check(vec![
        Case::new(
            "each instance has its own props and time",
            scene,
            // Frame 2: the strip's first probe starts, and its second and the root's wait. Frame 5:
            // the strip is 3 frames in, its second probe 2 and moved 1 px, and the root's probe 1
            // and moved 2 px, each covering the ones before it.
            Expect::PerFrame(vec![
                row([[0, 50, 0, 255]; 3]),
                row([[30, 50, 0, 255], [20, 51, 0, 255], [10, 99, 0, 255]]),
            ]),
        )
        .frames([2.0, 5.0])
        .tolerance(0),
    ]);
}

#[test]
fn a_hidden_clip_records_nothing_upstream() {
    let composition = scene(
        (4, 4),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "plate" } },
            { "id": 2, "type": "clip", "in": { "source": 1 }, "params": { "start": 5 } },
        ]),
    )
    .frames(12)
    .into();
    let media = TestMedia::default().image("plate", 4, 4, vec![255; 4 * 4 * 4]);
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.bind_frame(&composition, 2, &media).unwrap();
    assert_eq!(
        media.image_requests(),
        0,
        "a hidden clip resolved its source"
    );
    renderer.bind_frame(&composition, 6, &media).unwrap();
    assert!(media.image_requests() > 0);
}

/// A 16 × 4 root playing frames 4 to 7 of a 12-frame template whose 2 × 2 square is at x = frame.
fn mover_scene() -> Scene {
    Scene(json!({
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
            { "id": 1, "type": "clip", "params": {
                "composition": "mover", "start": 10, "duration": 4, "offset": 4
            } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    }))
}

#[test]
fn a_clip_bounds_an_animated_source_over_its_own_frames() {
    const BLANK: &str = "................";
    let square = |row: &'static str| Expect::AlphaRows(vec![BLANK, row, row, BLANK]);
    check(vec![
        Case::new(
            "the square is whole at the clip's first, second and last frames",
            mover_scene(),
            Expect::PerFrame(vec![
                square("....##.........."),
                square(".....##........."),
                square(".......##......."),
            ]),
        )
        .frames([10.0, 11.0, 13.0])
        .tolerance(0),
    ]);

    // A frame bounds the square to this frame alone, rounded out to a size class narrower than this
    // canvas, and follows it.
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let mut wide = mover_scene();
    wide.0["render_settings"]["width"] = json!(64);
    wide.0["compositions"]["mover"]["render_settings"] = json!({ "width": 64, "height": 4 });
    let wide = Composition::from(wide);
    for (frame, square) in [(10.0, 4..6), (13.0, 7..9)] {
        render(&mut renderer, &wide, frame, &TestMedia::default());
        let domain = renderer
            .recorded_domain(&wide, NodeId::new(1))
            .expect("the clip recorded");
        let rect = domain.data_rect;
        assert!(
            !domain.is_full_frame()
                && rect.width <= 16
                && rect.x <= square.start
                && rect.x + rect.width as i32 >= square.end,
            "frame {frame}: the clip is not bounded to the square at {square:?}: {rect:?}"
        );
    }
}

/// A root at `fps[0]` playing a template at `fps[1]`, and so on, where the last template shows the
/// 30 fps video `v`, all at default timing.
fn video_chain(fps: &[u32]) -> Scene {
    let template = |fps: u32, nodes: Value| {
        json!({
            "timeline": { "fps": fps, "duration_frames": 600 },
            "render_settings": { "width": 1, "height": 1 },
            "nodes": nodes,
            "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
        })
    };
    let playing = |node: Value| {
        let node = json!({ "id": 1, "type": node["type"], "params": node["params"] });
        json!([node, { "id": 2, "type": "media_output" }])
    };
    let clip = |name: String| playing(json!({ "type": "clip", "params": { "composition": name } }));
    let video =
        playing(json!({ "type": "media_in", "params": { "kind": "video", "source": "v" } }));
    let last = fps.len() - 1;
    let templates: serde_json::Map<_, _> = (1..=last)
        .map(|index| {
            let nodes = if index == last {
                video.clone()
            } else {
                clip(format!("t{}", index + 1))
            };
            (format!("t{index}"), template(fps[index], nodes))
        })
        .collect();
    let mut root = template(fps[0], clip("t1".to_string()));
    root["compositions"] = Value::Object(templates);
    Scene(root)
}

#[test]
fn clips_at_other_frame_rates_land_on_the_source_frame() {
    // Source frames are predicted over the whole range without rendering. The rendered ones are
    // around the exact multiples where a rate conversion can round down, frames where converting
    // through a float instead of a rational does, and one ordinary frame.
    const RENDERED: [f64; 12] = [
        0.0, 4.0, 5.0, 6.0, 10.0, 41.0, 82.0, 99.0, 149.0, 164.0, 169.0, 199.0,
    ];
    let media = TestMedia::default().numbered_video("v");
    // Root frame n shows source frame n * ratio.0 / ratio.1.
    let rows: [(&[u32], (u32, u32)); 3] = [
        (&[30, 24], (1, 1)),
        (&[30, 24, 60], (1, 1)),
        (&[25, 30, 24], (6, 5)),
    ];
    let mut cases = Vec::new();
    for (fps, ratio) in rows {
        let source = |frame: u32| frame * ratio.0 / ratio.1;
        let composition = Composition::from(video_chain(fps));
        for frame in 0..200 {
            let required = collect_frame_requirements(&composition, &media, frame).unwrap();
            assert_eq!(
                required.videos[0].frames,
                [source(frame)],
                "{fps:?} predicted at {frame}"
            );
        }
        let shown = RENDERED
            .iter()
            .map(|&frame| Expect::Uniform([source(frame as u32) as u8, 0, 0, 255]));
        cases.push(
            Case::new(
                format!("{fps:?}"),
                video_chain(fps),
                Expect::PerFrame(shown.collect()),
            )
            .media(media.clone())
            .frames(RENDERED)
            .tolerance(0),
        );
    }
    check(cases);
}

/// A 64 × 64 root that plays a template of `width` × 64 holding a small shape and `reader`.
fn wide_template_scene(width: u32, reader: &str) -> Composition {
    let mut scene = scene(
        (64, 64),
        json!([{ "id": 1, "type": "clip", "params": { "composition": "wide" } }]),
    );
    scene.0["compositions"] = json!({
        "wide": {
            "render_settings": { "width": width, "height": 64 },
            "nodes": [
                { "id": 1, "type": "shape", "params": {
                    "width": 8, "height": 8, "position": [32, 32], "fill_paint": "#ff0000"
                } },
                { "id": 2, "type": reader },
                { "id": 3, "type": "media_output" }
            ],
            "connections": [
                { "from_node": 1, "to_node": 2, "to_port": "source" },
                { "from_node": 2, "to_node": 3, "to_port": "source" }
            ]
        }
    });
    scene.into()
}

#[test]
fn a_template_canvas_over_the_texture_limit_fails_a_full_frame_read() {
    let gpu = gpu();
    let max = gpu
        .renderer()
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    let width = max + 1;
    let media = TestMedia::default();
    let mut renderer = gpu.renderer();

    let error = renderer
        .render_frame_at(&wide_template_scene(width, "resize"), 0.0, &media)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Resize"), "{error}");
    assert!(
        error.contains(&format!(
            "a {width}x64 canvas is larger than this device's {max}-pixel texture limit"
        )),
        "{error}"
    );

    render(
        &mut renderer,
        &wide_template_scene(width, "opacity"),
        0.0,
        &media,
    );
}

#[test]
fn a_root_canvas_over_the_texture_limit_fails_a_render() {
    let gpu = gpu();
    let max = gpu
        .renderer()
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    let width = max + 1;
    let composition: Composition = scene(
        (width, 64),
        json!([{ "id": 1, "type": "shape", "params": {
            "width": 8, "height": 8, "position": [32, 32], "fill_paint": "#ff0000"
        } }]),
    )
    .into();
    let error = gpu
        .renderer()
        .render_frame_at(&composition, 0.0, &TestMedia::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("MediaOutput"), "{error}");
    assert!(
        error.contains(&format!(
            "a {width}x64 canvas is larger than this device's {max}-pixel texture limit"
        )),
        "{error}"
    );
}

#[test]
fn a_root_canvas_over_the_texture_limit_fails_the_output_size_check() {
    let gpu = gpu();
    let renderer = gpu.renderer();
    let max = renderer
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    let width = max + 1;
    let composition: Composition = scene(
        (width, 64),
        json!([{ "id": 1, "type": "shape", "params": { "width": 8, "height": 8 } }]),
    )
    .into();
    let error = renderer
        .check_output_size(&composition)
        .unwrap_err()
        .to_string();
    assert!(error.contains("MediaOutput"), "{error}");
    assert!(
        error.contains(&format!(
            "a {width}x64 canvas is larger than this device's {max}-pixel texture limit"
        )),
        "{error}"
    );
}

#[test]
fn an_oversized_texture_is_blamed_on_the_node_that_acquires_it() {
    let gpu = gpu();
    let max = gpu
        .renderer()
        .gpu_renderer()
        .device
        .limits()
        .max_texture_dimension_2d;
    let width = max + 1;
    let shape = |position: u32| {
        json!({ "type": "shape", "params": {
            "width": 8, "height": 8, "position": [position, 32], "fill_paint": "#ff0000"
        } })
    };
    let mut nodes = vec![shape(16), shape(40)];
    for (node, id) in nodes.iter_mut().zip(1..) {
        node["id"] = json!(id);
    }
    nodes.push(json!({
        "id": 3, "type": "boolean", "params": {}, "in": { "a": 1, "b": 2 }
    }));
    let composition: Composition = scene((width, 64), Value::Array(nodes)).into();
    let error = gpu
        .renderer()
        .render_frame_at(&composition, 0.0, &TestMedia::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("Boolean node 3"), "{error}");
    assert!(
        error.contains(&format!(
            "a {width}x64 canvas is larger than this device's {max}-pixel texture limit"
        )),
        "{error}"
    );
}
