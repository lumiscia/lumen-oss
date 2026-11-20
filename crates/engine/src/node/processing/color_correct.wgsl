struct ColorCorrectParams {
    white_balance: vec4<f32>,
    lift: vec4<f32>,
    gamma: vec4<f32>,
    gain: vec4<f32>,
    offset: vec4<f32>,
    contrast: f32,
    pivot: f32,
    curved: u32,
    // The point count of each curve: master, red, green, blue.
    curve_counts: vec4<u32>,
    // Each curve's points, 16 to a curve: x, y and the slope there.
    curve_points: array<vec4<f32>, 64>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ColorCorrectParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

// `base` to the power `exponent`, where zero to the power zero is one.
fn power(base: vec3<f32>, exponent: vec3<f32>) -> vec3<f32> {
    return select(pow(base, exponent), select(vec3<f32>(0.0), vec3<f32>(1.0), exponent == vec3<f32>(0.0)), base == vec3<f32>(0.0));
}

// The monotone cubic through curve `curve`, flat before its first point and after its last.
fn apply_curve(curve: u32, x: f32) -> f32 {
    let count = params.curve_counts[curve];
    if (count == 0u) {
        return x;
    }
    let first = params.curve_points[curve * 16u];
    let last = params.curve_points[curve * 16u + count - 1u];
    if (count == 1u || x <= first.x) {
        return first.y;
    }
    if (x >= last.x) {
        return last.y;
    }
    var segment = 0u;
    while (segment + 2u < count && x >= params.curve_points[curve * 16u + segment + 1u].x) {
        segment += 1u;
    }
    let a = params.curve_points[curve * 16u + segment];
    let b = params.curve_points[curve * 16u + segment + 1u];
    let width = b.x - a.x;
    let t = (x - a.x) / width;
    let t2 = t * t;
    let t3 = t2 * t;
    return (2.0 * t3 - 3.0 * t2 + 1.0) * a.y
        + (t3 - 2.0 * t2 + t) * width * a.z
        + (3.0 * t2 - 2.0 * t3) * b.y
        + (t3 - t2) * width * b.z;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = unpremultiply(textureLoad(input_tex, vec2<i32>(id.xy), 0));
    var rgb = color.rgb * params.white_balance.rgb;
    rgb = params.gain.rgb * (rgb + params.lift.rgb * (vec3<f32>(1.0) - rgb)) + params.offset.rgb;
    rgb = power(max(rgb, vec3<f32>(0.0)), 1.0 / params.gamma.rgb);
    rgb = params.pivot * power(max(rgb, vec3<f32>(0.0)) / params.pivot, vec3<f32>(params.contrast));
    if (params.curved != 0u) {
        var encoded = clamp(lumen_linear_to_srgb(rgb), vec3<f32>(0.0), vec3<f32>(1.0));
        encoded = vec3<f32>(apply_curve(0u, encoded.r), apply_curve(0u, encoded.g), apply_curve(0u, encoded.b));
        encoded = vec3<f32>(apply_curve(1u, encoded.r), apply_curve(2u, encoded.g), apply_curve(3u, encoded.b));
        rgb = lumen_srgb_to_linear(encoded);
    }
    textureStore(output_tex, vec2<i32>(id.xy), lumen_finite(premultiply(vec4<f32>(rgb, color.a))));
}
