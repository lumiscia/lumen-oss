struct TextGlobals {
    target_size: vec2<f32>,
    px_range: f32,
    glyph_count: u32,
    paint_bounds: vec4<f32>,
}

struct GlyphInstance {
    rect: vec4<f32>,
    uv_rect: vec4<f32>,
    color: vec4<f32>,
    mode: u32,
    flags: u32,
    // The glyph's own transform, applied before the text's when `GLYPH_TRANSFORMED` is set.
    offset: vec2<f32>,
    linear: mat2x2<f32>,
}

// Bit of `GlyphInstance.flags`; keep in sync with `GLYPH_TRANSFORMED` in `gpu.rs`.
const GLYPH_TRANSFORMED: u32 = 1u;
// Keep in sync with `GLYPH_CLUSTER_FOLLOWER` and `GLYPH_CLUSTER_SHIFT` in `gpu.rs`.
const GLYPH_CLUSTER_FOLLOWER: u32 = 2u;
const GLYPH_CLUSTER_SHIFT: u32 = 8u;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) texel: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) mode: u32,
    @location(3) @interpolate(flat) uv_rect: vec4<f32>,
    // Inverse of the glyph's own transform, as the columns of its linear part and its offset.
    @location(4) @interpolate(flat) inverse_linear: vec4<f32>,
    @location(5) @interpolate(flat) inverse_offset: vec2<f32>,
    @location(6) @interpolate(flat) transformed: u32,
    @location(7) @interpolate(flat) instance: u32,
}

@group(0) @binding(0) var<uniform> globals: TextGlobals;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
@group(0) @binding(3) var<storage, read> glyphs: array<GlyphInstance>;
struct TextStyle {
    fill: Paint,
    transform: DrawableTransform,
    motion: DrawableMotion,
}

@group(0) @binding(4) var<uniform> style: TextStyle;

// Requires the engine's alpha, paint and drawable transform chunks prepended.
fn text_paint_at(pixel: vec2<f32>) -> vec4<f32> {
    let local01 = clamp(
        (pixel - globals.paint_bounds.xy) / max(globals.paint_bounds.zw, vec2<f32>(1.0)),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    return sample_paint(style.fill, pixel, local01);
}

// The untransformed position under a canvas pixel of the glyph `in` draws, undoing the
// text's transform and then the glyph's own.
fn glyph_to_local(in: VertexOut, pixel: vec2<f32>) -> vec2<f32> {
    let point = drawable_to_local(style.transform, pixel);
    let inverse = mat2x2<f32>(in.inverse_linear.xy, in.inverse_linear.zw);
    return select(point, inverse * point + in.inverse_offset, in.transformed != 0u);
}

// Averages the paint over the canvas pixel; paint follows the text, so each sample
// is evaluated at its untransformed position.
fn text_paint_aa(in: VertexOut) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    for (var y = 0u; y < 4u; y = y + 1u) {
        for (var x = 0u; x < 4u; x = x + 1u) {
            let offset = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * 0.25 - vec2<f32>(0.5);
            color += text_paint_at(glyph_to_local(in, in.position.xy + offset));
        }
    }
    return color * 0.0625;
}

// The atlas texel of `instance` under the untransformed position `local`.
fn instance_texel(instance: GlyphInstance, local: vec2<f32>) -> vec2<f32> {
    return instance.uv_rect.xy
        + (local - instance.rect.xy) * (instance.uv_rect.zw - instance.uv_rect.xy) / instance.rect.zw;
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    if (instance_index >= globals.glyph_count || style.transform.opacity <= 0.0
        || glyphs[instance_index].color.a <= 0.0
        || (glyphs[instance_index].flags & GLYPH_CLUSTER_FOLLOWER) != 0u) {
        var out: VertexOut;
        out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.texel = vec2<f32>(0.0);
        out.color = vec4<f32>(0.0);
        out.mode = 0u;
        out.uv_rect = vec4<f32>(0.0);
        out.inverse_linear = vec4<f32>(1.0, 0.0, 0.0, 1.0);
        out.inverse_offset = vec2<f32>(0.0);
        out.transformed = 0u;
        out.instance = 0u;
        return out;
    }
    let glyph = glyphs[instance_index];
    let transformed = (glyph.flags & GLYPH_TRANSFORMED) != 0u;
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    // Scaled, rotated or moved raster glyphs are filtered, so their quads grow by a texel
    // to fit the filter's falloff; raster texels are one layout pixel.
    let filtered = drawable_is_linear(style.transform) || transformed;
    // A cluster lead covers the rects of all its instances.
    let followers = glyph.flags >> GLYPH_CLUSTER_SHIFT;
    var rect = glyph.rect;
    for (var follower = 1u; follower <= followers; follower = follower + 1u) {
        let other = glyphs[instance_index + follower].rect;
        let min_corner = min(rect.xy, other.xy);
        let max_corner = max(rect.xy + rect.zw, other.xy + other.zw);
        rect = vec4<f32>(min_corner, max_corner - min_corner);
    }
    let pad = select(0.0, 1.0, filtered && (glyph.mode != 2u || followers > 0u));
    let local = rect.xy - vec2<f32>(pad) + corner * (rect.zw + vec2<f32>(2.0 * pad));
    let moved = select(local, glyph.linear * local + glyph.offset, transformed);
    let pixel = drawable_to_canvas(style.transform, moved);
    let clip = vec2<f32>(
        (pixel.x / globals.target_size.x) * 2.0 - 1.0,
        1.0 - (pixel.y / globals.target_size.y) * 2.0,
    );

    var out: VertexOut;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.texel = mix(glyph.uv_rect.xy - vec2<f32>(pad), glyph.uv_rect.zw + vec2<f32>(pad), corner);
    if (followers > 0u) {
        out.texel = instance_texel(glyph, local);
    }
    out.color = glyph.color;
    out.mode = glyph.mode;
    out.uv_rect = glyph.uv_rect;
    // A glyph scaled to nothing has no opacity, so is never drawn; its inverse is finite.
    let inverse = mat2x2<f32>(
        vec2<f32>(glyph.linear[1].y, -glyph.linear[0].y),
        vec2<f32>(-glyph.linear[1].x, glyph.linear[0].x),
    ) * (1.0 / determinant(glyph.linear));
    out.inverse_linear = vec4<f32>(inverse[0], inverse[1]);
    out.inverse_offset = -(inverse * glyph.offset);
    out.transformed = select(0u, 1u, transformed);
    out.instance = instance_index;
    return out;
}

// Raster texel, transparent outside the glyph's atlas slot.
fn raster_texel(coord: vec2<i32>, uv_rect: vec4<f32>) -> vec4<f32> {
    let slot_min = vec2<i32>(uv_rect.xy);
    let slot_max = vec2<i32>(uv_rect.zw) - vec2<i32>(1);
    let inside = all(coord >= slot_min) && all(coord <= slot_max);
    let texel = textureLoad(atlas_texture, clamp(coord, slot_min, slot_max), 0);
    return select(vec4<f32>(0.0), texel, inside);
}

fn raster_bilinear(texel: vec2<f32>, uv_rect: vec4<f32>) -> vec4<f32> {
    let base = floor(texel - vec2<f32>(0.5));
    let frac = texel - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = raster_texel(p0, uv_rect);
    let c10 = raster_texel(p0 + vec2<i32>(1, 0), uv_rect);
    let c01 = raster_texel(p0 + vec2<i32>(0, 1), uv_rect);
    let c11 = raster_texel(p0 + vec2<i32>(1, 1), uv_rect);
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

fn median3(value: vec3<f32>) -> f32 {
    return max(min(value.r, value.g), min(max(value.r, value.g), value.b));
}

// The instances of a cluster that fades, composited at full alpha so that they fade as one,
// at the untransformed position `local`. The cluster's lead instance draws them all.
// `texel_width` is how far the lead's texel moves per screen pixel.
fn cluster_color(in: VertexOut, local: vec2<f32>, texel_width: vec2<f32>, paint: vec4<f32>) -> vec4<f32> {
    let lead = glyphs[in.instance];
    let lead_scale = (lead.uv_rect.zw - lead.uv_rect.xy) / lead.rect.zw;
    let filtered = drawable_is_linear(style.transform) || in.transformed != 0u;
    var color = vec4<f32>(0.0);
    for (var member = 0u; member <= lead.flags >> GLYPH_CLUSTER_SHIFT; member = member + 1u) {
        let glyph = glyphs[in.instance + member];
        let texel = instance_texel(glyph, local);
        var source = vec4<f32>(0.0);
        if (glyph.mode == 2u) {
            let inside = all(local >= glyph.rect.xy) && all(local <= glyph.rect.xy + glyph.rect.zw);
            let field = textureSampleLevel(
                atlas_texture,
                atlas_sampler,
                texel / vec2<f32>(textureDimensions(atlas_texture)),
                0.0,
            );
            let scale = (glyph.uv_rect.zw - glyph.uv_rect.xy) / glyph.rect.zw;
            let px_range = max(
                0.5 * dot(vec2<f32>(globals.px_range), lead_scale / (texel_width * scale)),
                1.0,
            );
            let coverage = clamp(px_range * (median3(field.rgb) - 0.5) + 0.5, 0.0, 1.0);
            source = paint * select(0.0, coverage, inside);
        } else {
            let raster = select(
                raster_texel(vec2<i32>(floor(texel)), glyph.uv_rect),
                raster_bilinear(texel, glyph.uv_rect),
                filtered,
            );
            source = select(paint * raster.a, raster * paint.a, glyph.mode == 1u);
        }
        color = source + color * (1.0 - source.a);
    }
    return color * in.color.a;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Raster glyphs map 1:1 onto atlas texels unless scaled or rotated. Distance fields
    // are filtered, and sampled outside the branch because derivatives need uniform flow.
    let raster = select(
        textureLoad(atlas_texture, vec2<i32>(floor(in.texel)), 0),
        raster_bilinear(in.texel, in.uv_rect),
        drawable_is_linear(style.transform) || in.transformed != 0u,
    );
    let field = textureSample(
        atlas_texture,
        atlas_sampler,
        in.texel / vec2<f32>(textureDimensions(atlas_texture)),
    );
    let texel_width = fwidth(in.texel);
    let screen_px_range = max(0.5 * dot(vec2<f32>(globals.px_range), 1.0 / texel_width), 1.0);
    // Paint follows the text, so it is evaluated at the untransformed pixel.
    let local = glyph_to_local(in, in.position.xy);
    var text_paint = text_paint_at(local);
    if (style.fill.paint_supersample != 0u) {
        text_paint = text_paint_aa(in);
    }
    if ((glyphs[in.instance].flags >> GLYPH_CLUSTER_SHIFT) != 0u) {
        return cluster_color(in, local, texel_width, text_paint);
    }
    // The atlas, the paint and the output are premultiplied. Opacity is applied once
    // the glyphs are composited, by the opacity pass below.
    if (in.mode == 1u) {
        return raster * (in.color.a * text_paint.a);
    }
    var coverage = raster.a;
    if (in.mode == 2u) {
        coverage = clamp(screen_px_range * (median3(field.rgb) - 0.5) + 0.5, 0.0, 1.0);
    }
    return text_paint * (coverage * in.color.a);
}

@group(0) @binding(5) var layer_texture: texture_2d<f32>;

fn layer_texel(coord: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(layer_texture));
    if (any(coord < vec2<i32>(0)) || any(coord >= size)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(layer_texture, coord, 0);
}

// Bilinear sample of the layer at a continuous coordinate, whose texel centres lie at
// integer + 0.5, and which is transparent beyond the layer.
fn layer_bilinear(coord: vec2<f32>) -> vec4<f32> {
    let base = floor(coord - vec2<f32>(0.5));
    let frac = coord - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = layer_texel(p0);
    let c10 = layer_texel(p0 + vec2<i32>(1, 0));
    let c01 = layer_texel(p0 + vec2<i32>(0, 1));
    let c11 = layer_texel(p0 + vec2<i32>(1, 1));
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

// Smear pass, for text that may move during the shutter. The layer holds the composited
// text, drawn still under `style.transform`, so overlapping glyphs and the paint's
// supersampling are already resolved. Each tap maps the layer's text to where it lies
// then, and the premultiplied layer is averaged over them. Where nothing moves, the
// layer is copied.
@fragment
fn fs_smear(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    if (style.motion.tap_count == 0u) {
        return textureLoad(layer_texture, vec2<i32>(position.xy), 0);
    }
    var color = vec4<f32>(0.0);
    for (var tap = 0u; tap < style.motion.tap_count; tap = tap + 1u) {
        let transform = style.motion.taps[tap];
        // A tap that collapses the text, or fades it out, shows none of it.
        if (transform.opacity <= 0.0) {
            continue;
        }
        let local = drawable_to_local(transform, position.xy);
        color += layer_bilinear(drawable_to_canvas(style.transform, local));
    }
    return color / f32(style.motion.tap_count);
}

// A triangle covering the target, for the passes that treat the composited text as a layer.
@vertex
fn vs_fullscreen(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// Opacity pass: drawn over the composited text with a blend that multiplies the target by
// the source alpha, so overlapping glyphs fade as one layer.
@fragment
fn fs_opacity() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, style.transform.opacity);
}
