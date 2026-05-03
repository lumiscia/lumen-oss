struct TextParams {
    color: vec4<f32>,
    position: vec2<f32>,
    font_size: f32,
    max_width: f32,
    content_len: u32,
    line_count: u32,
    alignment_horizontal: u32,
    alignment_vertical: u32,
    _pad: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: TextParams;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<storage, read> text_chars: array<u32>;

fn upper(code: u32) -> u32 {
    if (code >= 97u && code <= 122u) {
        return code - 32u;
    }
    return code;
}

fn glyph_row(code_raw: u32, row: u32) -> u32 {
    let code = upper(code_raw);
    if (code == 32u) { return 0u; }
    if (code == 46u) {
        return select(0u, 4u, row == 6u);
    }
    if (code == 44u) {
        return select(0u, select(4u, 8u, row == 6u), row >= 5u);
    }
    if (code == 33u) {
        return array<u32, 7>(4u, 4u, 4u, 4u, 4u, 0u, 4u)[row];
    }
    if (code == 45u) {
        return select(0u, 31u, row == 3u);
    }
    if (code == 48u) { return array<u32, 7>(14u, 17u, 19u, 21u, 25u, 17u, 14u)[row]; }
    if (code == 49u) { return array<u32, 7>(4u, 12u, 4u, 4u, 4u, 4u, 14u)[row]; }
    if (code == 50u) { return array<u32, 7>(14u, 17u, 1u, 2u, 4u, 8u, 31u)[row]; }
    if (code == 51u) { return array<u32, 7>(30u, 1u, 1u, 14u, 1u, 1u, 30u)[row]; }
    if (code == 52u) { return array<u32, 7>(2u, 6u, 10u, 18u, 31u, 2u, 2u)[row]; }
    if (code == 53u) { return array<u32, 7>(31u, 16u, 16u, 30u, 1u, 1u, 30u)[row]; }
    if (code == 54u) { return array<u32, 7>(14u, 16u, 16u, 30u, 17u, 17u, 14u)[row]; }
    if (code == 55u) { return array<u32, 7>(31u, 1u, 2u, 4u, 8u, 8u, 8u)[row]; }
    if (code == 56u) { return array<u32, 7>(14u, 17u, 17u, 14u, 17u, 17u, 14u)[row]; }
    if (code == 57u) { return array<u32, 7>(14u, 17u, 17u, 15u, 1u, 1u, 14u)[row]; }
    if (code == 65u) { return array<u32, 7>(14u, 17u, 17u, 31u, 17u, 17u, 17u)[row]; }
    if (code == 66u) { return array<u32, 7>(30u, 17u, 17u, 30u, 17u, 17u, 30u)[row]; }
    if (code == 67u) { return array<u32, 7>(14u, 17u, 16u, 16u, 16u, 17u, 14u)[row]; }
    if (code == 68u) { return array<u32, 7>(30u, 17u, 17u, 17u, 17u, 17u, 30u)[row]; }
    if (code == 69u) { return array<u32, 7>(31u, 16u, 16u, 30u, 16u, 16u, 31u)[row]; }
    if (code == 70u) { return array<u32, 7>(31u, 16u, 16u, 30u, 16u, 16u, 16u)[row]; }
    if (code == 71u) { return array<u32, 7>(14u, 17u, 16u, 23u, 17u, 17u, 15u)[row]; }
    if (code == 72u) { return array<u32, 7>(17u, 17u, 17u, 31u, 17u, 17u, 17u)[row]; }
    if (code == 73u) { return array<u32, 7>(14u, 4u, 4u, 4u, 4u, 4u, 14u)[row]; }
    if (code == 74u) { return array<u32, 7>(7u, 2u, 2u, 2u, 18u, 18u, 12u)[row]; }
    if (code == 75u) { return array<u32, 7>(17u, 18u, 20u, 24u, 20u, 18u, 17u)[row]; }
    if (code == 76u) { return array<u32, 7>(16u, 16u, 16u, 16u, 16u, 16u, 31u)[row]; }
    if (code == 77u) { return array<u32, 7>(17u, 27u, 21u, 21u, 17u, 17u, 17u)[row]; }
    if (code == 78u) { return array<u32, 7>(17u, 25u, 21u, 19u, 17u, 17u, 17u)[row]; }
    if (code == 79u) { return array<u32, 7>(14u, 17u, 17u, 17u, 17u, 17u, 14u)[row]; }
    if (code == 80u) { return array<u32, 7>(30u, 17u, 17u, 30u, 16u, 16u, 16u)[row]; }
    if (code == 81u) { return array<u32, 7>(14u, 17u, 17u, 17u, 21u, 18u, 13u)[row]; }
    if (code == 82u) { return array<u32, 7>(30u, 17u, 17u, 30u, 20u, 18u, 17u)[row]; }
    if (code == 83u) { return array<u32, 7>(15u, 16u, 16u, 14u, 1u, 1u, 30u)[row]; }
    if (code == 84u) { return array<u32, 7>(31u, 4u, 4u, 4u, 4u, 4u, 4u)[row]; }
    if (code == 85u) { return array<u32, 7>(17u, 17u, 17u, 17u, 17u, 17u, 14u)[row]; }
    if (code == 86u) { return array<u32, 7>(17u, 17u, 17u, 17u, 17u, 10u, 4u)[row]; }
    if (code == 87u) { return array<u32, 7>(17u, 17u, 17u, 21u, 21u, 21u, 10u)[row]; }
    if (code == 88u) { return array<u32, 7>(17u, 17u, 10u, 4u, 10u, 17u, 17u)[row]; }
    if (code == 89u) { return array<u32, 7>(17u, 17u, 10u, 4u, 4u, 4u, 4u)[row]; }
    if (code == 90u) { return array<u32, 7>(31u, 1u, 2u, 4u, 8u, 16u, 31u)[row]; }
    return array<u32, 7>(31u, 17u, 21u, 21u, 21u, 17u, 31u)[row];
}

fn glyph_alpha(code: u32, pos: vec2<f32>, scale: f32) -> f32 {
    if (code == 0u) {
        return 0.0;
    }
    let glyph_x = i32(floor(pos.x / scale));
    let glyph_y_i = i32(floor(pos.y / scale));
    if (glyph_x < 0 || glyph_x >= 5 || glyph_y_i < 0 || glyph_y_i >= 7) {
        return 0.0;
    }
    let glyph_y = u32(glyph_y_i);
    let row_bits = glyph_row(code, glyph_y);
    let bit = 4u - u32(glyph_x);
    return select(0.0, 1.0, ((row_bits >> bit) & 1u) == 1u);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_tex);
    if (id.x >= dims.x || id.y >= dims.y) {
        return;
    }

    let font_size = max(params.font_size, 1.0);
    let line_height = font_size * 1.25;
    let scale = max(floor(font_size / 8.0), 1.0);
    let glyph_width = scale * 5.0;
    let char_width = glyph_width + max(scale * 1.5, 1.0);
    let content_len = max(params.content_len, 1u);
    let requested_width = select(f32(dims.x), params.max_width, params.max_width > 0.0);
    let chars_per_line = max(u32(floor(requested_width / char_width)), 1u);
    let line_count = max((content_len + chars_per_line - 1u) / chars_per_line, 1u);
    let block_height = f32(line_count) * line_height;
    let vertical_origin = params.position.y + select(
        0.0,
        select(
            max((f32(dims.y) - block_height) * 0.5, 0.0),
            max(f32(dims.y) - block_height, 0.0),
            params.alignment_vertical == 2u,
        ),
        params.alignment_vertical != 0u,
    );

    let pixel = vec2<f32>(f32(id.x) + 0.5, f32(id.y) + 0.5);
    let rel_y = pixel.y - vertical_origin;
    let line_index = u32(floor(rel_y / line_height));
    if (line_index >= line_count) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }

    let remaining = content_len - min(line_index * chars_per_line, content_len);
    let chars_on_line = min(chars_per_line, remaining);
    let line_width = f32(chars_on_line) * char_width;
    let horizontal_origin = params.position.x + select(
        0.0,
        select(
            max((f32(dims.x) - line_width) * 0.5, 0.0),
            max(f32(dims.x) - line_width, 0.0),
            params.alignment_horizontal == 2u,
        ),
        params.alignment_horizontal != 0u,
    );
    let rel_x = pixel.x - horizontal_origin;
    let char_index = u32(floor(rel_x / char_width));
    let in_line_y = rel_y - f32(line_index) * line_height;
    let in_char_x = rel_x - f32(char_index) * char_width;
    let text_index = line_index * chars_per_line + char_index;
    let code = select(0u, text_chars[text_index], text_index < content_len && text_index < 4096u);
    let alpha = params.color.a * glyph_alpha(code, vec2<f32>(in_char_x, in_line_y), scale);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(params.color.rgb * alpha, alpha));
}
