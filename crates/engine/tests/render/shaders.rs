//! What a `wgsl_shader` node binds for its shader, and what it reports when the shader is wrong.

use lumen_engine::{composition::Composition, gpu::RenderTarget};
use serde_json::{Value, json};

use crate::color::{linear, opaque, srgb8};
use crate::support::{BLACK, Case, Expect, Scene, TestMedia, check, gpu, render, scene};

/// A shader that writes `color`, a WGSL `vec4<f32>` expression, to every pixel.
pub fn fill_shader(color: &str) -> String {
    format!(
        "@compute @workgroup_size(8, 8, 1) fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) \
         {{ textureStore(output, vec2<i32>(id.xy), {color}); }}"
    )
}

fn background(id: u32, color: [u8; 4]) -> Value {
    json!({ "id": id, "type": "background", "params": { "paint": color, "width": 8, "height": 4 } })
}

/// An 8 × 4 scene whose last node is a shader reading `inputs`, which are backgrounds numbered from 1.
pub fn shader_scene(shader: &str, uniforms: Value, inputs: &[[u8; 4]]) -> Scene {
    let mut nodes: Vec<Value> = (0..inputs.len() as u32)
        .map(|index| background(index + 1, inputs[index as usize]))
        .collect();
    let sources: Vec<u32> = (1..=inputs.len() as u32).collect();
    nodes.push(json!({
        "id": 10, "type": "wgsl_shader", "in": { "source": sources },
        "params": { "shader": shader, "uniforms": uniforms }
    }));
    scene((8, 4), json!(nodes))
}

/// The time and resolution as sRGB codes on the left half, and the frame on the right.
const GLOBALS_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    var codes = vec3<f32>(lumen.time, lumen.resolution / 255.0);
    if (id.x >= 4u) {
        codes = vec3<f32>(f32(lumen.frame) / 255.0, 0.0, 0.0);
    }
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(lumen_srgb_to_linear(codes), 1.0));
}
";

/// Green where the camera matrices are the identity and the position is zero, red elsewhere.
const NO_CAMERA_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let identity = mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0), vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0), vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
    let still = all(lumen.view[0] == identity[0]) && all(lumen.view[1] == identity[1])
        && all(lumen.view[2] == identity[2]) && all(lumen.view[3] == identity[3])
        && all(lumen.projection[0] == identity[0]) && all(lumen.projection[1] == identity[1])
        && all(lumen.projection[2] == identity[2]) && all(lumen.projection[3] == identity[3])
        && all(lumen.camera_position == vec3<f32>(0.0));
    textureStore(output, vec2<i32>(id.xy), select(vec4<f32>(1.0, 0.0, 0.0, 1.0), vec4<f32>(0.0, 1.0, 0.0, 1.0), still));
}
";

const VEC3_SHADER: &str = "
struct Params {
    amount: f32,
    tint: vec3<f32>,
    alpha: f32,
    count: u32,
}
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let blue = lumen_srgb_to_linear(vec3<f32>(params.amount * f32(params.count))).x;
    let color = vec4<f32>(params.tint.xy, blue, 1.0) * params.alpha;
    textureStore(output, vec2<i32>(id.xy), color);
}
";

const THREE_INPUTS_SHADER: &str = "
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

const DECLARED_SHADER: &str = "
struct ShaderParams {
    values: vec4<f32>,
}
@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ShaderParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    textureStore(output_tex, vec2<i32>(id.xy), color + params.values);
}
";

const JSON_UNIFORMS_SHADER: &str = "diagnostic(off, derivative_uniformity);

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

#[test]
fn shader_abi() {
    check(vec![
        Case::new(
            "lumen globals follow the frame and the timeline's fps",
            shader_scene(GLOBALS_SHADER, json!({}), &[BLACK])
                .frames(12)
                .fps(24.0),
            Expect::PerFrame(vec![
                Expect::Probes(vec![(0, 0, [0, 8, 4, 255]), (7, 3, [0, 0, 0, 255])]),
                Expect::Probes(vec![(0, 0, [64, 8, 4, 255]), (7, 3, [6, 0, 0, 255])]),
            ]),
        )
        .frames([0.0, 6.0]),
        // The tint arrives as linear light, which the output encodes back to its codes.
        Case::new(
            "vec3 uniforms keep the alignment and expressions their time",
            shader_scene(
                VEC3_SHADER,
                json!({ "tint": "#ff8000", "amount": 0.125, "count": 2, "alpha": "=time * 2" }),
                &[BLACK],
            )
            .frames(12)
            .fps(24.0),
            Expect::Uniform([128, 64, 32, 128]),
        )
        .frames([6.0]),
        Case::new(
            "three inputs and the shared sampler",
            shader_scene(
                THREE_INPUTS_SHADER,
                json!({ "gain": [1, 2, 4] }),
                &[[200, 0, 0, 255], [0, 50, 0, 255], [0, 0, 25, 255]],
            ),
            Expect::Uniform(opaque([linear(200), 2.0 * linear(50), 4.0 * linear(25)])),
        ),
        Case::new(
            "a shader that declares its own bindings",
            shader_scene(
                DECLARED_SHADER,
                json!({ "values": [0.5, 0, 0, 0] }),
                &[BLACK],
            ),
            Expect::Uniform([srgb8(0.5), 0, 0, 255]),
        ),
        Case::new(
            "without a camera the matrices are the identity and the position zero",
            shader_scene(NO_CAMERA_SHADER, json!({}), &[BLACK]),
            Expect::Uniform([0, 255, 0, 255]),
        ),
        Case::new(
            "uniforms from composition JSON",
            shader_scene(
                JSON_UNIFORMS_SHADER,
                json!({
                    "strength": 0.25, "center": [64, "=frame * 2"], "tint": "#ff0000", "steps": 2
                }),
                &[BLACK],
            )
            .frames(12)
            .fps(24.0),
            Expect::Uniform(opaque([0.5, 64.0 / 255.0, 12.0 / 255.0])),
        )
        .frames([6.0]),
    ]);
}

#[test]
fn shader_errors_name_the_problem_and_leave_the_renderer_usable() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let write_only_input = "
@group(0) @binding(0) var input_tex: texture_storage_2d<rgba16float, write>;
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(input_tex, vec2<i32>(id.xy), vec4<f32>(1.0));
}
";
    // What the shader declares is checked when the composition is built.
    let matrix = "struct P { m: mat4x4<f32> }\n@group(0) @binding(1) var<uniform> p: P;";
    for (shader, expected) in [
        (matrix.to_string(), "unsupported type `mat4x4<f32>`"),
        (
            DECLARED_SHADER.replace("rgba16float", "rgba8unorm"),
            "`texture_storage_2d<rgba8unorm, ...>` is not supported; declare \
             `texture_storage_2d<rgba16float, write>`",
        ),
    ] {
        let error = lumen_engine::json::parse_value(&shader_scene(&shader, json!({}), &[BLACK]).0)
            .expect_err(expected)
            .to_string();
        assert!(error.contains(expected), "{expected}: {error}");
    }
    let rows = [
        (
            DECLARED_SHADER,
            json!({ "tint": [1, 0, 0, 0] }),
            "uniforms.tint: `ShaderParams` has no such field",
        ),
        (
            DECLARED_SHADER,
            json!({ "values": [1, 0, 0] }),
            "uniforms.values: vec4<f32> needs 4 components",
        ),
        (
            &fill_shader("vec4<f32>(1.0"),
            json!({}),
            "GPU validation failed",
        ),
        (write_only_input, json!({}), "GPU validation failed"),
    ];
    for (shader, uniforms, expected) in rows {
        let composition = Composition::from(shader_scene(shader, uniforms, &[BLACK]));
        let error = renderer
            .render(&composition, 0.0, &media, RenderTarget::Owned)
            .expect_err(expected);
        assert_ne!(error.code(), "render.gpu", "{expected}");
        let error = error.to_string();
        assert!(error.contains(expected), "{expected}: {error}");
    }

    let valid = Composition::from(shader_scene(
        &fill_shader("vec4<f32>(1.0)"),
        json!({}),
        &[[0; 4]],
    ));
    let frame = render(&mut renderer, &valid, 0.0, &media);
    assert!(frame.rgba.iter().all(|&byte| byte == 255));
}

#[test]
fn shader_source_keys_follow_time_remap_branches() {
    let source = format!(
        "=if(frame < 5, '{}', '{}')",
        fill_shader("vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
        fill_shader("vec4<f32>(0.0, 1.0, 0.0, 1.0)")
    );
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
    let (red, green) = ([255, 0, 0, 255], [0, 255, 0, 255]);
    let halves_are = |left, right| {
        Expect::Probes(vec![
            (0, 0, left),
            (3, 3, left),
            (4, 0, right),
            (7, 3, right),
        ])
    };
    check(vec![
        Case::new(
            "each branch of a time remap picks its own shader source",
            scene(
                (8, 4),
                json!([
                    background(1, BLACK),
                    { "id": 2, "type": "wgsl_shader", "in": { "source": 1 }, "params": { "shader": source } },
                    { "id": 3, "type": "time_remap", "in": { "source": 2 }, "params": {
                        "frame": "=if(frame < 2, 0, 9)"
                    } },
                    { "id": 4, "type": "time_remap", "in": { "source": 2 }, "params": {
                        "frame": "=if(frame % 2 == 0, 0, 9)"
                    } },
                    { "id": 5, "type": "wgsl_shader", "in": { "source": [3, 4] }, "params": {
                        "shader": halves
                    } },
                ]),
            )
            .frames(12),
            Expect::PerFrame(vec![
                halves_are(red, red),
                halves_are(red, green),
                halves_are(green, red),
                halves_are(green, green),
            ]),
        )
        .frames([0.0, 1.0, 2.0, 3.0]),
    ]);
}

fn with_shaders(mut scene: Scene, shaders: Value) -> Scene {
    scene.0["shaders"] = shaders;
    scene
}

#[test]
fn composition_shaders_render_as_their_inline_source_does() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let uniforms = json!({
        "strength": 0.25, "center": [64, "=frame * 2"], "tint": "#ff0000", "steps": 2
    });
    let inline = Composition::from(
        shader_scene(JSON_UNIFORMS_SHADER, uniforms.clone(), &[BLACK]).frames(12),
    );
    let named = Composition::from(with_shaders(
        shader_scene("shader:ripple", uniforms, &[BLACK]).frames(12),
        json!({ "ripple": JSON_UNIFORMS_SHADER }),
    ));
    for at in [0.0, 6.0] {
        let expected = render(&mut renderer, &inline, at, &media).rgba;
        assert_eq!(
            render(&mut renderer, &named, at, &media).rgba,
            expected,
            "frame {at}"
        );
    }

    let fills = json!({
        "red": fill_shader("vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
        "green": fill_shader("vec4<f32>(0.0, 1.0, 0.0, 1.0)"),
    });
    let picked = |shader: &str| {
        Composition::from(with_shaders(
            shader_scene(shader, json!({}), &[BLACK]).frames(2),
            fills.clone(),
        ))
    };
    let picks = picked("=if(frame < 1, 'shader:red', 'shader:green')");
    assert_eq!(
        render(&mut renderer, &picks, 0.0, &media).rgba[..4],
        [255, 0, 0, 255]
    );
    assert_eq!(
        render(&mut renderer, &picks, 1.0, &media).rgba[..4],
        [0, 255, 0, 255]
    );
    let missing = picked("=if(frame < 1, 'shader:red', 'shader:blue')");
    render(&mut renderer, &missing, 0.0, &media);
    let error = renderer
        .render(&missing, 1.0, &media, RenderTarget::Owned)
        .expect_err("`blue` is not a shader")
        .to_string();
    assert!(
        error.contains("node 10") && error.contains("no shader named `blue` in `shaders`"),
        "{error}"
    );
}

#[test]
fn identical_shader_nodes_record_without_hashing_their_source() {
    let copy = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(output, vec2<i32>(id.xy), textureLoad(input0, vec2<i32>(id.xy), 0));
}
";
    let mut nodes = vec![background(1, [10, 20, 30, 255])];
    nodes.extend((2..22).map(|id| {
        json!({ "id": id, "type": "wgsl_shader", "in": { "source": id - 1 }, "params": { "shader": copy } })
    }));
    let composition = Composition::from(scene((8, 4), json!(nodes)).frames(2));
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let before = renderer.shader_source_lookups();
    let frame = render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(frame.rgba[..4], [10, 20, 30, 255]);
    // The twenty nodes share one shader, which only its first lookup finds by its source.
    assert_eq!(renderer.shader_source_lookups() - before, 1);
    render(&mut renderer, &composition, 1.0, &media);
    assert_eq!(renderer.shader_source_lookups() - before, 1);
}

/// How far, times 1000, a colour uniform's decoded value is from the prelude decoding `ENCODED`.
const PARITY_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let prelude = lumen_srgb_to_linear(vec3<f32>(ENCODED));
    let difference = abs(uniforms.tint.rgb - prelude) * 1000.0;
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(difference, 1.0));
}

struct Uniforms {
    tint: vec4<f32>,
}
@group(0) @binding(1) var<uniform> uniforms: Uniforms;
";

#[test]
fn colour_uniforms_decode_past_black_and_white_as_the_prelude_does() {
    // Extrapolated colours reach the CPU decoder, which must mirror the curve about zero as
    // `lumen_srgb_to_linear` does, and keep white at exactly 1.
    let cases = [-0.5f64, -0.01, 0.0, 0.5, 1.0, 1.5]
        .map(|encoded| {
            let shader = PARITY_SHADER.replace("ENCODED", &format!("{encoded:.4}"));
            let tint = format!("=lerp(#000000, #ffffff, {encoded})");
            Case::new(
                format!("encoded {encoded}"),
                shader_scene(&shader, json!({ "tint": tint }), &[[0, 0, 0, 255]]),
                Expect::Uniform([0, 0, 0, 255]),
            )
        })
        .into();
    check(cases);
}

#[test]
fn shader_output_is_kept_in_the_working_space() {
    // Alpha past 1 is clamped and colour keeps its light; a NaN texel is transparent black. The
    // NaN is built from its bits, since a compiler that assumes finite math may fold 0 / 0.
    let cases = [
        (
            "alpha past one",
            "vec4<f32>(1.5, 0.5, 0.0, 2.0)",
            [255, 188, 0, 255],
        ),
        (
            "not a number",
            "vec4<f32>(bitcast<f32>(0x7fc00000u + lumen.frame))",
            [0; 4],
        ),
    ]
    .map(|(name, value, expected)| {
        Case::new(
            name,
            shader_scene(&fill_shader(value), json!({}), &[[0, 0, 0, 255]]),
            Expect::Uniform(expected),
        )
        .check_working_space()
    })
    .into();
    check(cases);
}

/// White at the pixel the world origin projects to through the camera, black elsewhere.
#[cfg(feature = "3d")]
const ORIGIN_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let clip = lumen.projection * lumen.view * vec4<f32>(0.0, 0.0, 0.0, 1.0);
    let ndc = clip.xy / clip.w;
    let pixel = vec2<i32>(floor(vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * lumen.resolution));
    let hit = all(vec2<i32>(id.xy) == pixel);
    textureStore(output, vec2<i32>(id.xy), select(vec4<f32>(0.0, 0.0, 0.0, 1.0), vec4<f32>(1.0), hit));
}
";

#[cfg(feature = "3d")]
#[test]
fn a_shader_projects_through_the_camera_of_its_scene() {
    // A 9 × 5 canvas has a single centre pixel, (4, 2).
    let projected = |scene_nodes: Value| {
        let mut nodes = vec![json!({
            "id": 1, "type": "background",
            "params": { "paint": [0, 0, 0, 255], "width": 9, "height": 5 }
        })];
        nodes.extend(scene_nodes.as_array().cloned().unwrap());
        nodes.push(json!({
            "id": 10, "type": "wgsl_shader", "in": { "source": [1], "camera": 2 },
            "params": { "shader": ORIGIN_SHADER }
        }));
        scene((9, 5), json!(nodes))
    };
    let at = |x: u32| Expect::Probes(vec![(x, 2, [255; 4]), (4 + 3 - x, 2, [0, 0, 0, 255])]);
    check(vec![
        Case::new(
            "the origin is at the centre of a camera looking at it down -Z",
            projected(json!([
                { "id": 2, "type": "camera_3d", "params": { "position": [0, 0, 5] } }
            ])),
            at(4),
        ),
        Case::new(
            "a camera moved along +X sees it to the left",
            projected(json!([
                { "id": 2, "type": "camera_3d", "params": { "position": [1, 0, 5] } }
            ])),
            at(3),
        ),
        Case::new(
            "the first camera of a scene wins, placed by the merge above it",
            projected(json!([
                { "id": 3, "type": "camera_3d", "params": { "position": [1, 0, 5] } },
                { "id": 4, "type": "camera_3d", "params": { "position": [-1, 0, 5] } },
                { "id": 2, "type": "merge_3d", "in": { "objects": [3, 4] }, "params": {} },
            ])),
            at(3),
        ),
        Case::new(
            "a scene without a camera is viewed as render_3d views it",
            projected(json!([{ "id": 2, "type": "merge_3d", "params": {} }])),
            at(4),
        ),
    ]);
}
