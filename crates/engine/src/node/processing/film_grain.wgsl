struct FilmGrainParams {
    // The amount, the roughness, and the response.
    grain: vec4<f32>,
    // Lattice cells per device pixel of the fine grain in `x` and of the coarse grain in `y`.
    cells: vec4<f32>,
    // Canvas pixel of the output's first texel.
    origin: vec4<i32>,
    // Whether the grain is monochrome, and the seed.
    modes: vec4<u32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: FilmGrainParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
// A uniform value in 0..1 has a standard deviation of 1 / sqrt(12).
const UNIFORM_SCALE: f32 = 3.4641016;
// Smoothstep interpolation of independent lattice values leaves a standard deviation of 26 / 35
// on average over a cell, which this restores to 1.
const INTERPOLATION_GAIN: f32 = 1.3461539;

fn pcg(value: u32) -> u32 {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// A value with zero mean and unit standard deviation for a lattice cell.
fn lattice_value(cell: vec2<i32>, salt: u32) -> f32 {
    let hash = pcg(bitcast<u32>(cell.x) ^ pcg(bitcast<u32>(cell.y) ^ pcg(salt)));
    return (f32(hash >> 8u) / 16777216.0 - 0.5) * UNIFORM_SCALE;
}

fn value_noise(point: vec2<f32>, salt: u32) -> f32 {
    let base = floor(point);
    let t = point - base;
    let s = t * t * (3.0 - 2.0 * t);
    let cell = vec2<i32>(base);
    let top = mix(lattice_value(cell, salt), lattice_value(cell + vec2<i32>(1, 0), salt), s.x);
    let bottom = mix(
        lattice_value(cell + vec2<i32>(0, 1), salt),
        lattice_value(cell + vec2<i32>(1, 1), salt),
        s.x,
    );
    return mix(top, bottom, s.y) * INTERPOLATION_GAIN;
}

// One octave of grain at the device pixel centred on `center`, averaged over 2x2 subsamples when
// its lattice cells are smaller than the pixel.
fn octave(center: vec2<f32>, cells_per_pixel: f32, salt: u32) -> f32 {
    if (cells_per_pixel <= 1.0) {
        return value_noise(center * cells_per_pixel, salt);
    }
    var sum = 0.0;
    for (var i = 0; i < 4; i++) {
        let offset = vec2<f32>(f32(i & 1) - 0.5, f32(i >> 1u) - 0.5) * 0.5;
        sum += value_noise((center + offset) * cells_per_pixel, salt);
    }
    return sum * 0.25;
}

// The grain of one channel: the fine and coarse octaves mixed at equal power, so that the
// roughness leaves the standard deviation alone.
fn channel_grain(center: vec2<f32>, channel: u32) -> f32 {
    let roughness = params.grain.y;
    var grain = 0.0;
    if (roughness < 1.0) {
        grain += sqrt(1.0 - roughness) * octave(center, params.cells.x, pcg(params.modes.y + channel * 2u));
    }
    if (roughness > 0.0) {
        grain += sqrt(roughness) * octave(center, params.cells.y, pcg(params.modes.y + channel * 2u + 1u));
    }
    return grain;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let source = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    if (params.grain.x == 0.0) {
        textureStore(output_tex, vec2<i32>(id.xy), source);
        return;
    }

    let center = vec2<f32>(vec2<i32>(id.xy) + params.origin.xy) + vec2<f32>(0.5);
    var grain = vec3<f32>(channel_grain(center, 0u));
    if (params.modes.x == 0u) {
        grain = vec3<f32>(grain.x, channel_grain(center, 1u), channel_grain(center, 2u));
    }

    let color = unpremultiply(source);
    let encoded = lumen_linear_to_srgb(color.rgb);
    let luma = clamp(dot(encoded, LUMA), 0.0, 1.0);
    let weight = mix(1.0, 4.0 * luma * (1.0 - luma), params.grain.z);
    let grained = max(encoded + params.grain.x * weight * grain, vec3<f32>(0.0));
    let rgb = lumen_srgb_to_linear(grained);
    textureStore(output_tex, vec2<i32>(id.xy), lumen_finite(premultiply(vec4<f32>(rgb, color.a))));
}
