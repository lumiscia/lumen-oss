// Stores a depth or normal target, drawn at the output's size, as a raster: each texel as it is
// where something was drawn, and the background elsewhere. Neither is colour, so the values go
// into the working space unchanged, and the output encodes them as it does light.

struct Params {
    background: vec4<f32>,
    // 0 when nothing was drawn, and the target holds nothing.
    drawn: u32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    var texel = params.background;
    if (params.drawn != 0u) {
        let drawn = textureLoad(input_tex, vec2<i32>(id.xy), 0);
        texel = select(params.background, drawn, drawn.a > 0.0);
    }
    textureStore(output_tex, vec2<i32>(id.xy), texel);
}
