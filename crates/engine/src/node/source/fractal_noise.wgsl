// One layer of detail. The noise-space point at texel `(0, 0)`'s centre is `cell + origin.xy`,
// split so that offsets far from the origin keep their precision.
struct NoiseOctave {
    // `xy`: the fractional part of the first texel's point. `z`: cells per pixel. `w`: weight.
    origin: vec4<f32>,
    cell: vec2<u32>,
    seed: u32,
    _pad: u32,
}

struct NoiseParams {
    // 64 unit gradients, two per entry, already turned by the evolution.
    gradients: array<vec4<f32>, 32>,
    octaves: array<NoiseOctave, 8>,
    octave_count: u32,
    turbulent: u32,
    contrast: f32,
    brightness: f32,
}

@group(0) @binding(0) var<uniform> params: NoiseParams;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba16float, write>;

// Chris Wellons' lowbias32, an integer mixer that is exact on every GPU.
fn noise_hash(value: u32) -> u32 {
    var h = value;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

fn noise_gradient(column: u32, row: u32) -> vec2<f32> {
    let index = noise_hash(column ^ row) >> 26u;
    let pair = params.gradients[index >> 1u];
    return select(pair.xy, pair.zw, (index & 1u) == 1u);
}

// 2D gradient noise with a quintic fade, in `-1..=1`.
fn gradient_noise(octave: NoiseOctave, texel: vec2<u32>) -> f32 {
    let point = octave.origin.xy + vec2<f32>(texel) * octave.origin.z;
    let whole = floor(point);
    let f = point - whole;
    let cell = octave.cell + vec2<u32>(vec2<i32>(whole));
    let left = noise_hash(cell.x ^ octave.seed);
    let right = noise_hash((cell.x + 1u) ^ octave.seed);
    let n00 = dot(noise_gradient(left, cell.y), f);
    let n10 = dot(noise_gradient(right, cell.y), f - vec2<f32>(1.0, 0.0));
    let n01 = dot(noise_gradient(left, cell.y + 1u), f - vec2<f32>(0.0, 1.0));
    let n11 = dot(noise_gradient(right, cell.y + 1u), f - vec2<f32>(1.0, 1.0));
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    // Unit gradients reach at most √½, so this spans the full range.
    let value = mix(mix(n00, n10, u.x), mix(n01, n11, u.x), u.y) * 1.4142135;
    return clamp(value, -1.0, 1.0);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    var sum = 0.0;
    for (var index = 0u; index < params.octave_count; index = index + 1u) {
        let octave = params.octaves[index];
        let value = gradient_noise(octave, id.xy);
        sum += octave.origin.w * select(value * 0.5, abs(value), params.turbulent == 1u);
    }
    let level = select(sum + 0.5, sum, params.turbulent == 1u);
    let shade = clamp((level - 0.5) * params.contrast + 0.5 + params.brightness, 0.0, 1.0);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(lumen_srgb_to_linear(vec3<f32>(shade)), 1.0));
}
