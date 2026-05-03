struct ResizeParams {
    size: vec2<u32>,
    mode: u32,
    sampling: u32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ResizeParams;
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
    let output_size = textureDimensions(output_tex);
    if (id.x >= output_size.x || id.y >= output_size.y) {
        return;
    }
    if (id.x >= params.size.x || id.y >= params.size.y) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }

    let input_size_u = textureDimensions(input_tex);
    let input_size = vec2<f32>(input_size_u);
    let dest_size = vec2<f32>(max(params.size, vec2<u32>(1u)));
    var source_coord = (vec2<f32>(id.xy) + vec2<f32>(0.5)) * input_size / dest_size - vec2<f32>(0.5);
    var transparent = false;

    if (params.mode == 1u) {
        let scale = min(dest_size.x / input_size.x, dest_size.y / input_size.y);
        let scaled = input_size * scale;
        let offset = (dest_size - scaled) * 0.5;
        let local = vec2<f32>(id.xy) + vec2<f32>(0.5) - offset;
        transparent = local.x < 0.0 || local.y < 0.0 || local.x >= scaled.x || local.y >= scaled.y;
        source_coord = local / scale - vec2<f32>(0.5);
    } else if (params.mode == 2u) {
        let scale = max(dest_size.x / input_size.x, dest_size.y / input_size.y);
        let crop_size = dest_size / scale;
        let crop_origin = (input_size - crop_size) * 0.5;
        source_coord = crop_origin + ((vec2<f32>(id.xy) + vec2<f32>(0.5)) / scale) - vec2<f32>(0.5);
    }

    if (transparent) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }

    let color = select(sample_linear(source_coord), sample_nearest(source_coord), params.sampling == 0u);
    textureStore(output_tex, vec2<i32>(id.xy), color);
}
