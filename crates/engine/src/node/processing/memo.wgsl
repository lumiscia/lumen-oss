struct Bounds {
    origin: vec2<i32>,
    size: vec2<u32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> source_bounds: Bounds;
@group(0) @binding(3) var<storage, read_write> output_bounds: Bounds;

// Copies the source's texels and the bounds published for them.
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x == 0u && id.y == 0u) {
        output_bounds = source_bounds;
    }
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    textureStore(output_tex, vec2<i32>(id.xy), textureLoad(source_tex, vec2<i32>(id.xy), 0));
}
