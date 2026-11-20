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

// Smoothstep-interpolated value noise at `point`, scaled by the interpolation's own standard
// deviation at that point so the result has unit standard deviation anywhere in a cell.
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
    let power = ((1.0 - s.x) * (1.0 - s.x) + s.x * s.x) * ((1.0 - s.y) * (1.0 - s.y) + s.y * s.y);
    return mix(top, bottom, s.y) * inverseSqrt(power);
}

// The lattice weights along one axis of the mean of two interpolated samples.
struct Taps {
    index: vec4<i32>,
    weight: vec4<f32>,
}

fn taps(first: f32, second: f32) -> Taps {
    let first_base = floor(first);
    let second_base = floor(second);
    let first_t = first - first_base;
    let second_t = second - second_base;
    let first_s = first_t * first_t * (3.0 - 2.0 * first_t);
    let second_s = second_t * second_t * (3.0 - 2.0 * second_t);
    let first_index = i32(first_base);
    let second_index = i32(second_base);
    return Taps(
        vec4<i32>(first_index, first_index + 1, second_index, second_index + 1),
        0.5 * vec4<f32>(1.0 - first_s, first_s, 1.0 - second_s, second_s),
    );
}

// The sum of the squared weights per lattice index, where taps on the same index add first.
fn energy(axis: Taps) -> f32 {
    var sum = 0.0;
    for (var i = 0; i < 4; i++) {
        for (var j = 0; j < 4; j++) {
            if (axis.index[i] == axis.index[j]) {
                sum += axis.weight[i] * axis.weight[j];
            }
        }
    }
    return sum;
}

// The mean of the four value-noise samples at the corners of a square around `center`, scaled to
// unit standard deviation. The samples share lattice values, so the scale comes from the combined
// weight of each lattice value; with a square pattern that factors into the two axes.
fn value_noise_2x2(center: vec2<f32>, half_extent: f32, salt: u32) -> f32 {
    let low = center - vec2<f32>(half_extent);
    let high = center + vec2<f32>(half_extent);
    let x = taps(low.x, high.x);
    let y = taps(low.y, high.y);
    var sum = 0.0;
    for (var i = 0; i < 4; i++) {
        for (var j = 0; j < 4; j++) {
            sum += x.weight[i] * y.weight[j] * lattice_value(vec2<i32>(x.index[i], y.index[j]), salt);
        }
    }
    return sum * inverseSqrt(energy(x) * energy(y));
}

// One octave of grain at the device pixel centred on `center`, averaged over 2x2 subsamples when
// its lattice cells are smaller than the pixel.
fn octave(center: vec2<f32>, cells_per_pixel: f32, salt: u32) -> f32 {
    if (cells_per_pixel <= 1.0) {
        return value_noise(center * cells_per_pixel, salt);
    }
    return value_noise_2x2(center * cells_per_pixel, 0.25 * cells_per_pixel, salt);
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
    if (params.grain.x == 0.0 || source.a <= ALPHA_EPSILON) {
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
