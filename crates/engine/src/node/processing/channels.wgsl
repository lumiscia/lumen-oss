struct ChannelsParams {
    // The pick for each output channel, in the order red, green, blue, alpha. 0 to 4 read
    // the source's red, green, blue, alpha and luma, 5 to 9 the other raster's, 10 is 0 and 11
    // is 1.
    picks: vec4<u32>,
    // Canvas pixels of the first texels of the output in `xy` and of the other raster in `zw`.
    origins: vec4<i32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var other_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: ChannelsParams;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba16float, write>;

const CHANNELS_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// Straight colour that keeps the alpha and any colour a nearly transparent pixel still holds, since
// a pick of `one` or of the other raster's alpha can make that pixel opaque.
fn channels_straight(color: vec4<f32>) -> vec4<f32> {
    if (color.a <= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, color.a);
    }
    return vec4<f32>(color.rgb / color.a, color.a);
}

fn channels_pick(source: vec4<f32>, other: vec4<f32>, pick: u32) -> f32 {
    switch (pick) {
        case 0u, 1u, 2u, 3u: { return source[pick]; }
        case 4u: { return dot(source.rgb, CHANNELS_LUMA); }
        case 5u, 6u, 7u, 8u: { return other[pick - 5u]; }
        case 9u: { return dot(other.rgb, CHANNELS_LUMA); }
        case 10u: { return 0.0; }
        default: { return 1.0; }
    }
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let source = channels_straight(textureLoad(source_tex, vec2<i32>(id.xy), 0));
    let pixel = vec2<i32>(id.xy) + params.origins.xy;
    let other = channels_straight(sample_or_transparent(other_tex, pixel, params.origins.zw));
    let picked = vec4<f32>(
        channels_pick(source, other, params.picks.x),
        channels_pick(source, other, params.picks.y),
        channels_pick(source, other, params.picks.z),
        channels_pick(source, other, params.picks.w),
    );
    textureStore(output_tex, vec2<i32>(id.xy), premultiply(picked));
}
