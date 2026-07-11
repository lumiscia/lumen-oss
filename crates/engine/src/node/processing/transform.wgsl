struct TransformParams {
    scale: vec2<f32>,
    translate: vec2<f32>,
    pivot: vec2<f32>,
    rotate_radians: f32,
    opacity: f32,
    sampling: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: TransformParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn read_pixel(coord: vec2<i32>) -> vec4<f32> {
    let size = textureDimensions(input_tex);
    if (coord.x < 0 || coord.y < 0 || coord.x >= i32(size.x) || coord.y >= i32(size.y)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(input_tex, coord, 0);
}

fn sample_nearest(coord: vec2<f32>) -> vec4<f32> {
    return read_pixel(vec2<i32>(floor(coord + vec2<f32>(0.5))));
}

fn sample_linear(coord: vec2<f32>) -> vec4<f32> {
    let base = floor(coord - vec2<f32>(0.5));
    let frac = coord - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = read_pixel(p0);
    let c10 = read_pixel(p0 + vec2<i32>(1, 0));
    let c01 = read_pixel(p0 + vec2<i32>(0, 1));
    let c11 = read_pixel(p0 + vec2<i32>(1, 1));
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let input_size = textureDimensions(input_tex);
    let default_pivot = vec2<f32>(input_size) * 0.5;
    let pivot = select(params.pivot, default_pivot, all(abs(params.pivot) <= vec2<f32>(0.000001)));
    let scale = select(params.scale, vec2<f32>(0.000001), abs(params.scale) <= vec2<f32>(0.000001));
    let centered = vec2<f32>(id.xy) - pivot - params.translate;
    let c = cos(-params.rotate_radians);
    let s = sin(-params.rotate_radians);
    let unrotated = vec2<f32>(
        centered.x * c - centered.y * s,
        centered.x * s + centered.y * c,
    );
    let source_coord = (unrotated / scale) + pivot;
    let color = select(sample_linear(source_coord), sample_nearest(source_coord), params.sampling == 0u);
    textureStore(output_tex, vec2<i32>(id.xy), color * params.opacity);
}
