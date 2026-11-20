struct DitherParams {
    // Premultiplied linear colours.
    ink: vec4<f32>,
    paper: vec4<f32>,
    // The canvas pixel of the source's first texel in `xy`, the matrix size in `z` and the cell
    // size in device pixels in `w`.
    origin_matrix_cell: vec4<i32>,
    // The number of levels, then the mode.
    levels_mode: vec4<u32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: DitherParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

const DITHER_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// The entry of the Bayer matrix of `size` cells, a power of two, at `cell`: each bit of the
// coordinates, lowest first, adds a base-4 digit, from the most significant down.
fn bayer(cell: vec2<u32>, size: u32) -> u32 {
    var value = 0u;
    for (var bit = 1u; bit < size; bit = bit << 1u) {
        let x = (cell.x & bit) != 0u;
        let y = (cell.y & bit) != 0u;
        value = value * 4u + (select(0u, 2u, x) ^ select(0u, 3u, y));
    }
    return value;
}

// `value`, 0 to 1, rounded to one of `levels` steps by `threshold`, 0 to 1.
fn quantize(value: vec3<f32>, threshold: f32) -> vec3<f32> {
    let steps = f32(params.levels_mode.x - 1u);
    return floor(clamp(value, vec3<f32>(0.0), vec3<f32>(1.0)) * steps + vec3<f32>(threshold)) / steps;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    if (color.a <= 0.0) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }

    let pixel = vec2<i32>(id.xy) + params.origin_matrix_cell.xy;
    let cell_size = params.origin_matrix_cell.w;
    let cell = vec2<i32>(floor(vec2<f32>(pixel) / f32(cell_size)));
    let matrix_size = u32(params.origin_matrix_cell.z);
    let wrapped = vec2<u32>(cell & vec2<i32>(i32(matrix_size) - 1));
    let threshold = (f32(bayer(wrapped, matrix_size)) + 0.5) / f32(matrix_size * matrix_size);

    let encoded = clamp(lumen_linear_to_srgb(unpremultiply(color).rgb), vec3<f32>(0.0), vec3<f32>(1.0));
    var result: vec4<f32>;
    if (params.levels_mode.y == 0u) {
        let level = quantize(vec3<f32>(1.0 - dot(encoded, DITHER_LUMA)), threshold).x;
        result = mix(params.paper, params.ink, level);
    } else {
        result = vec4<f32>(lumen_srgb_to_linear(quantize(encoded, threshold)), 1.0);
    }
    textureStore(output_tex, vec2<i32>(id.xy), lumen_finite(result * color.a));
}
