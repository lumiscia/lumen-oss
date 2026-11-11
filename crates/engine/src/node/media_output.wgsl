struct SourceOrigin {
    origin: vec2<i32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> source: SourceOrigin;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(3.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(floor(position.xy)) - source.origin;
    let source_size = vec2<i32>(textureDimensions(source_tex));
    if (any(pixel < vec2<i32>(0)) || any(pixel >= source_size)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(source_tex, pixel, 0);
}
