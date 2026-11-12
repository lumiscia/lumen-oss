//! `instance_3d`: copies of a scene placed by per-instance expressions, drawn as the same objects
//! placed by hand would be.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::support::{Case, Expect, TestMedia, check, gpu, render, scene};

const SIZE: (u32, u32) = (96, 96);
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn shape(id: u32, params: Value) -> Value {
    json!({ "id": id, "type": "shape_3d", "params": params })
}

fn instance(id: u32, scene: u32, params: Value) -> Value {
    json!({ "id": id, "type": "instance_3d", "params": params, "in": { "scene": scene } })
}

fn merge(id: u32, objects: &[u32]) -> Value {
    json!({ "id": id, "type": "merge_3d", "in": { "objects": objects } })
}

/// `scene_nodes`, the last of which is the scene, drawn by a `render_3d` filling the canvas.
fn render_3d(scene_nodes: Vec<Value>) -> crate::support::Scene {
    let last = scene_nodes.last().expect("a scene node")["id"].clone();
    let mut nodes = scene_nodes;
    nodes.push(
        json!({ "id": 100, "type": "render_3d", "params": { "environment": 0 },
        "in": { "scene": last } }),
    );
    scene(SIZE, json!(nodes))
}

/// A ground, a camera looking down at it from the front and a shadow-casting sun, with `objects`
/// on it, of which the nodes `shown` are in the scene.
fn on_ground(objects: Vec<Value>, shown: &[u32]) -> crate::support::Scene {
    let mut nodes = vec![
        shape(
            50,
            json!({ "shape": "plane", "size": [8, 1, 8], "roughness": 1 }),
        ),
        json!({ "id": 51, "type": "camera_3d", "params": {
            "position": [0, 5, 5], "look_at": true, "fov": 35 } }),
        json!({ "id": 52, "type": "light_3d", "params": {
            "light": "directional", "intensity": 2, "rotation": [-32, 40, 0],
            "cast_shadows": true, "shadow_softness": 0.02 } }),
    ];
    let ids: Vec<u32> = [50, 51, 52]
        .into_iter()
        .chain(shown.iter().copied())
        .collect();
    nodes.extend(objects);
    nodes.push(merge(90, &ids));
    render_3d(nodes)
}

/// The 3 × 3 grid of cubes the tests draw, each turned a little more than the one before.
fn grid_by_expression() -> crate::support::Scene {
    on_ground(
        vec![
            shape(
                1,
                json!({ "size": [0.6, 0.6, 0.6], "position": [0, 0.3, 0] }),
            ),
            instance(
                2,
                1,
                json!({
                    "count": 9,
                    "position": "=vec3(instance % 3 - 1, 0, floor(instance / 3) - 1)",
                    "rotation": "=vec3(0, instance * 15, 0)",
                }),
            ),
        ],
        &[2],
    )
}

fn grid_by_hand() -> crate::support::Scene {
    let cubes: Vec<Value> = (0..9_i32)
        .map(|i| {
            shape(
                (10 + i) as u32,
                json!({
                    "size": [0.6, 0.6, 0.6],
                    "position": [i % 3 - 1, 0.3, i / 3 - 1],
                    "rotation": [0, i * 15, 0],
                }),
            )
        })
        .collect();
    let ids: Vec<u32> = (10..19).collect();
    on_ground(cubes, &ids)
}

#[test]
fn a_grid_of_instances_draws_as_the_same_cubes_placed_by_hand() {
    check(vec![
        Case::new(
            "nine lit, shadowed cubes",
            grid_by_expression(),
            Expect::Matches(grid_by_hand().into()),
        ),
        Case::new(
            "which show ground, lit cubes and their shadows",
            grid_by_expression(),
            Expect::Custom(Box::new(|frames| {
                let shades: Vec<u8> = frames[0]
                    .rgba
                    .chunks(4)
                    .filter(|pixel| pixel[3] == 255)
                    .map(|pixel| pixel[0])
                    .collect();
                let count = |range: std::ops::Range<u8>| {
                    shades.iter().filter(|shade| range.contains(shade)).count()
                };
                assert!(count(0..20) > 300, "shadows cover {} pixels", count(0..20));
                assert!(
                    count(140..180) > 3000,
                    "the ground covers {} pixels",
                    count(140..180)
                );
                assert!(
                    count(180..255) > 300,
                    "lit cube tops cover {} pixels",
                    count(180..255)
                );
            })),
        ),
    ]);
}

#[test]
fn instances_of_a_merged_scene_keep_its_lights_once_and_place_its_objects() {
    // The input holds a cube and a sun; each copy of the cube is lit by the one sun.
    let with_light = |instanced: bool| {
        let mut nodes = vec![
            shape(1, json!({ "position": [0, 0.5, 0], "size": [0.5, 1, 0.5] })),
            json!({ "id": 2, "type": "light_3d", "params": {
                "light": "directional", "rotation": [-50, 40, 0] } }),
            merge(3, &[1, 2]),
        ];
        let objects: Vec<u32> = if instanced {
            nodes.push(instance(
                4,
                3,
                json!({ "count": 3, "position": "=vec3(instance - 1, 0, 0)" }),
            ));
            vec![4]
        } else {
            for i in 0..3_i32 {
                nodes.push(shape(
                    (10 + i) as u32,
                    json!({ "position": [i - 1, 0.5, 0], "size": [0.5, 1, 0.5] }),
                ));
            }
            nodes.push(json!({ "id": 20, "type": "light_3d", "params": {
                "light": "directional", "rotation": [-50, 40, 0] } }));
            vec![10, 11, 12, 20]
        };
        nodes.push(json!({ "id": 5, "type": "camera_3d", "params": {
            "position": [0, 2, 6], "look_at": true } }));
        let mut ids = objects;
        ids.push(5);
        nodes.push(merge(6, &ids));
        render_3d(nodes)
    };
    check(vec![Case::new(
        "three tall boxes",
        with_light(true),
        Expect::Matches(with_light(false).into()),
    )]);
}

#[test]
fn nested_instances_multiply_and_each_reads_its_own_index() {
    let nested = render_3d(vec![
        shape(1, json!({ "size": [0.3, 0.3, 0.3] })),
        instance(
            2,
            1,
            json!({ "count": 3, "position": "=vec3(instance * 0.5, 0, 0)" }),
        ),
        instance(
            3,
            2,
            json!({ "count": 2, "position": "=vec3(0, instance * 0.5 - 0.25, 0)" }),
        ),
    ]);
    let by_hand = render_3d({
        let mut nodes: Vec<Value> = (0..6)
            .map(|i| {
                shape(
                    10 + i,
                    json!({ "size": [0.3, 0.3, 0.3],
                            "position": [(i % 3) as f64 * 0.5, (i / 3) as f64 * 0.5 - 0.25, 0] }),
                )
            })
            .collect();
        nodes.push(merge(20, &[10, 11, 12, 13, 14, 15]));
        nodes
    });
    check(vec![Case::new(
        "3 by 2 boxes",
        nested,
        Expect::Matches(by_hand.into()),
    )]);
}

#[test]
fn no_instances_make_an_empty_scene() {
    check(vec![Case::new(
        "count 0",
        render_3d(vec![
            shape(1, json!({})),
            instance(2, 1, json!({ "count": 0 })),
        ]),
        Expect::Uniform(CLEAR),
    )]);
}

fn fox_media() -> TestMedia {
    TestMedia::default().model("fox.glb", include_bytes!("../fixtures/Fox.glb").to_vec())
}

fn fox_params() -> Value {
    json!({
        "source": "fox.glb",
        "position": [0, -0.8, 0],
        "rotation": [0, 90, 0],
        "scale": [0.012, 0.012, 0.012],
        "animation": "Walk",
    })
}

fn by_expression() -> crate::support::Scene {
    render_3d(vec![
        json!({ "id": 1, "type": "model_3d", "params": fox_params() }),
        instance(
            2,
            1,
            json!({ "count": 2, "position": "=vec3(instance * 1.2 - 0.6, 0, 0)" }),
        ),
    ])
    .frames(30)
    .fps(30.0)
}

#[test]
fn instances_of_a_skinned_model_share_its_pose_and_animate() {
    let by_hand = render_3d({
        let fox = |id: u32, x: f64| {
            let mut params = fox_params();
            params["position"] = json!([x, -0.8, 0]);
            json!({ "id": id, "type": "model_3d", "params": params })
        };
        vec![fox(10, -0.6), fox(11, 0.6), merge(12, &[10, 11])]
    })
    .frames(30)
    .fps(30.0);
    check(vec![
        Case::new(
            "two walking foxes at several frames",
            by_expression(),
            Expect::Matches(by_hand.into()),
        )
        .media(fox_media())
        .frames([0.0, 7.0, 15.0]),
        Case::new(
            "which pose differently at each of them",
            by_expression(),
            Expect::Custom(Box::new(|frames| {
                assert!(frames[0].rgba != frames[1].rgba);
                assert!(frames[1].rgba != frames[2].rgba);
                assert!(frames[0].rgba.iter().any(|&byte| byte > 0));
            })),
        )
        .media(fox_media())
        .frames([0.0, 7.0, 15.0]),
    ]);
}

#[test]
fn an_instance_node_stays_static_unless_its_params_read_the_frame() {
    let gpu = gpu();
    let media = TestMedia::default();
    let skipped = |position: &str| {
        let composition: Composition = render_3d(vec![
            shape(1, json!({})),
            instance(2, 1, json!({ "count": 4, "position": position })),
        ])
        .frames(4)
        .fps(4.0)
        .into();
        let mut renderer = gpu.renderer();
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|frame| render(&mut renderer, &composition, f64::from(frame), &media).rgba)
            .collect();
        (frames, renderer.skipped_pass_count())
    };

    let (still, skipped_still) = skipped("=vec3(instance - 1.5, 0, instance_count * -0.1)");
    assert!(still.iter().all(|frame| *frame == still[0]));
    assert!(
        skipped_still >= 3 * 2,
        "frames after the first are kept, skipping {skipped_still} passes"
    );
    let (moving, skipped_moving) = skipped("=vec3(instance - 1.5, sin(time * 3), 0)");
    assert_eq!(
        skipped_moving, 0,
        "a placement that reads time renders every frame"
    );
    assert!(moving[0] != moving[1]);
}

fn parse_error(nodes: Vec<Value>) -> String {
    match lumen_engine::json::parse_value(&render_3d(nodes).0) {
        Ok(_) => panic!("the composition parses"),
        Err(error) => format!("{error:#}"),
    }
}

fn render_error(nodes: Vec<Value>) -> String {
    let composition: Composition = render_3d(nodes).into();
    gpu()
        .renderer()
        .render(
            &composition,
            0.0,
            &TestMedia::default(),
            RenderTarget::Owned,
        )
        .expect_err("the render fails")
        .to_string()
}

#[test]
fn a_count_outside_the_limit_is_rejected_where_it_is_written() {
    for count in [json!(100_001), json!(-1)] {
        let error = parse_error(vec![
            shape(1, json!({})),
            instance(2, 1, json!({ "count": count })),
        ]);
        assert!(
            error.contains("must be an integer from 0 to 100000"),
            "{error}"
        );
    }
    let error = render_error(vec![
        shape(1, json!({})),
        instance(2, 1, json!({ "count": "=100001" })),
    ]);
    assert!(
        error.contains("100001 instances is out of range"),
        "{error}"
    );
}

#[test]
fn nested_copies_past_the_object_limit_fail_before_the_inner_ones_are_placed() {
    // The inner placement fails if it is ever evaluated.
    let error = render_error(vec![
        shape(1, json!({})),
        instance(
            2,
            1,
            json!({ "count": 1000, "position": "=vec3(if(instance >= 0, glyph, 0), 0, 0)" }),
        ),
        instance(3, 2, json!({ "count": 1000 })),
    ]);
    assert!(error.contains("node 2"), "{error}");
    assert!(
        error.contains("more than 100000 objects, copied by the instance_3d nodes holding them"),
        "{error}"
    );
}

#[test]
fn an_empty_scene_is_not_placed() {
    // The placement fails if it is ever evaluated.
    let composition: Composition = render_3d(vec![
        shape(1, json!({})),
        instance(2, 1, json!({ "count": 0 })),
        instance(
            3,
            2,
            json!({ "count": 1000, "position": "=vec3(if(instance >= 0, glyph, 0), 0, 0)" }),
        ),
    ])
    .into();
    gpu()
        .renderer()
        .render(
            &composition,
            0.0,
            &TestMedia::default(),
            RenderTarget::Owned,
        )
        .expect("the render succeeds");
}

#[test]
fn copies_of_a_scene_past_the_object_limit_fail_before_they_are_made() {
    let error = render_error(vec![
        shape(1, json!({})),
        shape(2, json!({})),
        merge(3, &[1, 2]),
        instance(4, 3, json!({ "count": 60_000 })),
    ]);
    assert!(error.contains("node 4"), "{error}");
    assert!(
        error.contains(
            "60000 instances of a scene of 2 objects make a scene of more than 100000 objects"
        ),
        "{error}"
    );
}

#[test]
fn instance_nodes_nested_too_deep_fail_naming_the_node() {
    let mut nodes = vec![shape(1, json!({}))];
    for id in 2..=11 {
        nodes.push(instance(id, id - 1, json!({ "count": 1 })));
    }
    let error = render_error(nodes);
    assert!(error.contains("nested more than 8 deep"), "{error}");
}

#[test]
fn an_expression_failing_for_one_instance_names_the_node_param_and_instance() {
    let error = render_error(vec![
        shape(1, json!({})),
        instance(
            2,
            1,
            json!({ "count": 5, "position": "=vec3(if(instance == 3, glyph, 0), 0, 0)" }),
        ),
    ]);
    assert!(error.contains("node 2"), "{error}");
    assert!(error.contains("position"), "{error}");
    assert!(error.contains("instance 3"), "{error}");
}

#[test]
fn the_instance_index_is_only_readable_in_an_instance_nodes_placement() {
    let error = render_error(vec![shape(
        1,
        json!({ "position": "=vec3(instance, 0, 0)" }),
    )]);
    assert!(error.contains("`instance` is only available"), "{error}");
}

#[test]
fn copies_placed_once_draw_as_copies_placed_again_under_a_moving_parent() {
    let gpu = gpu();
    let media = TestMedia::default();
    let composition: Composition = scene(
        SIZE,
        json!([
            json!({ "id": 51, "type": "camera_3d", "params": {
                "position": [0, 2, 4], "look_at": true, "fov": 40 } }),
            shape(1, json!({ "size": [0.3, 0.3, 0.3] })),
            instance(
                2,
                1,
                json!({ "count": 3, "position": "=vec3(instance * 0.5 - 0.5, 0, 0)",
                        "rotation": "=vec3(0, instance * 20, 0)" }),
            ),
            instance(
                3,
                2,
                json!({ "count": 2, "position": "=vec3(0, instance * 0.5 - 0.25, 0)" }),
            ),
            json!({ "id": 4, "type": "merge_3d", "in": { "objects": [3] },
                    "params": { "rotation": "=vec3(0, frame * 25, frame * 10)" } }),
            merge(5, &[51, 4]),
            json!({ "id": 100, "type": "render_3d", "params": { "environment": 0 },
                    "in": { "scene": 5 } }),
            json!({ "id": 101, "type": "motion_blur", "params": { "samples": 4, "shutter": 180 },
                    "in": { "source": 100 } }),
        ]),
    )
    .frames(4)
    .fps(4.0)
    .into();
    let mut kept = gpu.renderer();
    let frames: Vec<Vec<u8>> = (0..4)
        .map(|frame| {
            let at = f64::from(frame);
            let placed_again = render(&mut gpu.renderer(), &composition, at, &media).rgba;
            let frame = render(&mut kept, &composition, at, &media).rgba;
            assert!(
                frame == placed_again,
                "frame {at} draws as a first frame would"
            );
            frame
        })
        .collect();
    assert!(frames[0] != frames[1], "the parent moves the copies");
}
