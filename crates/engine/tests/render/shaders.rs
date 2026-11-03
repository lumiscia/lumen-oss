//! What a `wgsl_shader` node binds for its shader, and what it reports when the shader is wrong.

use lumen_engine::{composition::Composition, gpu::AlphaMode};
use serde_json::{Value, json};

use crate::support::{Case, Expect, Scene, TestMedia, check, gpu, render, scene};

const BLACK: [u8; 4] = [0, 0, 0, 255];

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

const GLOBALS_SHADER: &str = "
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let color = vec4<f32>(lumen.time, lumen.resolution / 255.0, f32(lumen.frame) / 255.0);
    textureStore(output, vec2<i32>(id.xy), color);
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
    let color = vec4<f32>(params.tint.xy, params.amount * f32(params.count), params.alpha);
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
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

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

/// An opaque blue input and a half-transparent red one, converted to straight alpha, behind a
/// shader that returns the second.
fn straight_input_scene() -> Scene {
    let read_second = fill_shader("textureLoad(input1, vec2<i32>(id.xy), 0)");
    scene(
        (8, 4),
        json!([
            background(1, [0, 0, 255, 255]),
            background(2, [255, 0, 0, 128]),
            { "id": 3, "type": "alpha_premultiply", "in": { "source": 2 }, "params": { "mode": "unpremultiply" } },
            { "id": 4, "type": "wgsl_shader", "in": { "source": [1, 3] }, "params": { "shader": read_second } },
        ]),
    )
}

#[test]
fn shader_abi() {
    check(vec![
        Case::new(
            "lumen globals follow the frame and the timeline's fps",
            shader_scene(GLOBALS_SHADER, json!({}), &[BLACK])
                .frames(12)
                .fps(24.0),
            Expect::PerFrame(vec![
                Expect::Uniform([0, 8, 4, 0]),
                Expect::Uniform([64, 8, 4, 6]),
            ]),
        )
        .frames([0.0, 6.0]),
        Case::new(
            "vec3 uniforms keep the alignment and expressions their time",
            shader_scene(
                VEC3_SHADER,
                json!({ "tint": "#ff8000", "amount": 0.125, "count": 2, "alpha": "=time * 2" }),
                &[BLACK],
            )
            .frames(12)
            .fps(24.0),
            Expect::Uniform([255, 128, 64, 128]),
        )
        .frames([6.0]),
        Case::new(
            "three inputs and the shared sampler",
            shader_scene(
                THREE_INPUTS_SHADER,
                json!({ "gain": [1, 2, 4] }),
                &[[200, 0, 0, 255], [0, 50, 0, 255], [0, 0, 25, 255]],
            ),
            Expect::Uniform([200, 100, 100, 255]),
        ),
        Case::new(
            "a shader that declares its own bindings",
            shader_scene(
                DECLARED_SHADER,
                json!({ "values": [0.5, 0, 0, 0] }),
                &[BLACK],
            ),
            Expect::Uniform([128, 0, 0, 255]),
        ),
        Case::new(
            "a straight input is premultiplied for the shader",
            straight_input_scene(),
            Expect::Uniform([128, 0, 0, 128]),
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
            Expect::Uniform([128, 64, 12, 255]),
        )
        .frames([6.0]),
    ]);
}

#[test]
fn shader_output_is_premultiplied() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let raster = renderer
        .render_frame_at(
            &Composition::from(straight_input_scene()),
            0.0,
            &TestMedia::default(),
        )
        .unwrap();
    assert_eq!(raster.alpha_mode, AlphaMode::Premultiplied);
}

#[test]
fn shader_errors_name_the_problem_and_leave_the_renderer_usable() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    let media = TestMedia::default();
    let oversized_fields: String = (0..4097)
        .map(|index| format!("f{index}: vec4<f32>,"))
        .collect();
    let oversized = format!(
        "struct Big {{ {oversized_fields} }}\n@group(0) @binding(1) var<uniform> big: Big;\n{}",
        fill_shader("big.f0")
    );
    let write_only_input = "
@group(0) @binding(0) var input_tex: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(input_tex, vec2<i32>(id.xy), vec4<f32>(1.0));
}
";
    let matrix = "struct P { m: mat4x4<f32> }\n@group(0) @binding(1) var<uniform> p: P;";
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
        (matrix, json!({}), "unsupported type `mat4x4<f32>`"),
        (&oversized, json!({}), "limited to 65536 bytes"),
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
            .render_frame_at(&composition, 0.0, &media)
            .expect_err(expected)
            .to_string();
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
