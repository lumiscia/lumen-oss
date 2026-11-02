struct TextGlobals {
    target_size: vec2<f32>,
    px_range: f32,
    glyph_count: u32,
}

struct GlyphInstance {
    rect: vec4<f32>,
    uv_rect: vec4<f32>,
    opacity: f32,
    // With `GLYPH_PARTNER`, the instance drawn over this one.
    partner: u32,
    padding: vec2<u32>,
    // The render mode in the bits of `GLYPH_MODE_MASK`, and the index into `paints` from
    // `GLYPH_PAINT_SHIFT` up.
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
// Keep in sync with `GLYPH_RANGE_SHIFT` and `GLYPH_RANGE_MASK` in `gpu.rs`.
const GLYPH_RANGE_SHIFT: u32 = 2u;
const GLYPH_RANGE_MASK: u32 = 0xfu;
// Keep in sync with `GLYPH_PARTNER` in `gpu.rs`.
const GLYPH_PARTNER: u32 = 64u;
// Keep in sync with `GLYPH_MODE_MASK` and `GLYPH_PAINT_SHIFT` in `gpu.rs`.
const GLYPH_MODE_MASK: u32 = 0xffu;
const GLYPH_PAINT_SHIFT: u32 = 8u;
// Keep in sync with `GLYPH_MODE_RECT`, the `RECT_*` constants and `RECT_WAVY_AMPLITUDE` in
// `gpu.rs`.
const GLYPH_MODE_RECT: u32 = 3u;
const RECT_SHAPE_SHIFT: u32 = 8u;
const RECT_BOX: u32 = 0u;
const RECT_DOUBLE: u32 = 2u;
const RECT_DOTTED: u32 = 3u;
const RECT_DASHED: u32 = 4u;
const RECT_WAVY: u32 = 5u;
const RECT_JOINED_LEFT: u32 = 4u;
const RECT_JOINED_RIGHT: u32 = 8u;
const RECT_WAVY_AMPLITUDE: f32 = 1.5;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // The atlas texel, or a rect's untransformed position, which the target's viewport offset
    // does not move.
    @location(0) texel: vec2<f32>,
    @location(1) @interpolate(flat) opacity: f32,
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
    transform: DrawableTransform,
    motion: DrawableMotion,
}

@group(0) @binding(4) var<uniform> style: TextStyle;

// A paint and the box, as `[x, y, width, height]`, that its relative coordinates map to `0..1`.
struct TextPaint {
    paint: Paint,
    bounds: vec4<f32>,
}

@group(0) @binding(6) var<storage, read> paints: array<TextPaint>;

// Requires the engine's alpha, paint and drawable transform chunks prepended.
fn text_paint_at(fill: u32, pixel: vec2<f32>) -> vec4<f32> {
    let bounds = paints[fill].bounds;
    let local01 = clamp(
        (pixel - bounds.xy) / max(bounds.zw, vec2<f32>(1.0)),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    return sample_paint(paints[fill].paint, pixel, local01);
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
fn text_paint_aa(in: VertexOut, fill: u32) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    for (var y = 0u; y < 4u; y = y + 1u) {
        for (var x = 0u; x < 4u; x = x + 1u) {
            let offset = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * 0.25 - vec2<f32>(0.5);
            color += text_paint_at(fill, glyph_to_local(in, in.position.xy + offset));
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
        || glyphs[instance_index].opacity <= 0.0
        || (glyphs[instance_index].flags & GLYPH_CLUSTER_FOLLOWER) != 0u) {
        var out: VertexOut;
        out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.texel = vec2<f32>(0.0);
        out.opacity = 0.0;
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
    if ((glyph.mode & GLYPH_MODE_MASK) == GLYPH_MODE_RECT) {
        return rect_vertex(glyph, corner, instance_index);
    }
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
    let pad = select(0.0, 1.0, filtered && ((glyph.mode & GLYPH_MODE_MASK) != 2u || followers > 0u));
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
    out.opacity = glyph.opacity;
    out.mode = glyph.mode & GLYPH_MODE_MASK;
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

// How far a rect's line pattern reaches beyond its stroke, which is `thickness` tall.
fn rect_reach(shape: u32, thickness: f32) -> f32 {
    if (shape == RECT_DOUBLE) {
        return thickness;
    }
    if (shape == RECT_WAVY) {
        return thickness * RECT_WAVY_AMPLITUDE;
    }
    return 0.0;
}

// A corner of a rect instance's quad, grown by its pattern's reach and a canvas pixel of
// antialiasing.
fn rect_vertex(rect: GlyphInstance, corner: vec2<f32>, instance_index: u32) -> VertexOut {
    let canvas_pixel = 1.0 / max(drawable_axis_scale(style.transform), vec2<f32>(0.001));
    let grow = canvas_pixel + vec2<f32>(0.0, rect_reach(rect.flags >> RECT_SHAPE_SHIFT, rect.rect.w));
    let local = rect.rect.xy - grow + corner * (rect.rect.zw + 2.0 * grow);
    let pixel = drawable_to_canvas(style.transform, local);
    var out: VertexOut;
    out.position = vec4<f32>(
        (pixel.x / globals.target_size.x) * 2.0 - 1.0,
        1.0 - (pixel.y / globals.target_size.y) * 2.0,
        0.0,
        1.0,
    );
    out.texel = local;
    out.opacity = rect.opacity;
    out.mode = GLYPH_MODE_RECT;
    out.uv_rect = vec4<f32>(0.0);
    out.inverse_linear = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    out.inverse_offset = vec2<f32>(0.0);
    out.transformed = 0u;
    out.instance = instance_index;
    return out;
}

// Signed distance from `p` to a box of half size `half` centred on the origin.
fn box_distance(p: vec2<f32>, half: vec2<f32>) -> f32 {
    let q = abs(p) - half;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
}

// Direction in which `box_distance` grows at `p`.
fn box_gradient(p: vec2<f32>, half: vec2<f32>) -> vec2<f32> {
    let q = abs(p) - half;
    let outside = max(q, vec2<f32>(0.0));
    let direction = select(
        select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), q.x > q.y),
        outside / max(length(outside), 0.000001),
        any(q > vec2<f32>(0.0)),
    );
    return direction * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
}

// Coverage of a rect instance at the untransformed position `local`. Each shape is measured in
// untransformed pixels, then scaled to canvas pixels across its edge, so it keeps its shape
// under any transform. A line's stroke runs along `rect`'s width, and its pattern repeats from
// `uv_rect.y`.
fn rect_coverage(rect: GlyphInstance, local: vec2<f32>) -> f32 {
    let shape = rect.flags >> RECT_SHAPE_SHIFT;
    let half = rect.rect.zw * 0.5;
    let p = local - rect.rect.xy - half;
    if (shape == RECT_BOX) {
        let radius = min(rect.uv_rect.x, min(half.x, half.y));
        let distance = box_distance(p, half - radius) - radius;
        let scale = drawable_distance_scale(style.transform, box_gradient(p, half - radius));
        return clamp(0.5 - distance * scale, 0.0, 1.0);
    }
    let thickness = rect.rect.w;
    let u = local.x - rect.uv_rect.y;
    let v = p.y;
    // Distance in untransformed pixels, and the direction in which it grows.
    var normal = vec2<f32>(0.0, 1.0);
    var distance = abs(v) - half.y;
    if (shape == RECT_DOUBLE) {
        distance = abs(abs(v) - thickness) - half.y;
    } else if (shape == RECT_DOTTED) {
        let period = 2.0 * thickness;
        let along = vec2<f32>(u - (round((u - half.y) / period) * period + half.y), v);
        distance = length(along) - half.y;
        normal = select(vec2<f32>(0.0, 1.0), normalize(along), dot(along, along) > 0.000001);
    } else if (shape == RECT_DASHED) {
        let dash = 3.0 * thickness;
        let period = 5.0 * thickness;
        let along = vec2<f32>(u - (round((u - 0.5 * dash) / period) * period + 0.5 * dash), v);
        distance = box_distance(along, vec2<f32>(0.5 * dash, half.y));
        normal = box_gradient(along, vec2<f32>(0.5 * dash, half.y));
    } else if (shape == RECT_WAVY) {
        let amplitude = RECT_WAVY_AMPLITUDE * thickness;
        let frequency = 6.2831853 / (4.0 * amplitude);
        let slope = amplitude * frequency * cos(frequency * u);
        distance = abs(v - amplitude * sin(frequency * u)) / sqrt(1.0 + slope * slope) - half.y;
        normal = normalize(vec2<f32>(-slope, 1.0));
    }
    distance = distance * drawable_distance_scale(style.transform, normal);
    // The ends are antialiased, except where they join another rect.
    let end_scale = drawable_distance_scale(style.transform, vec2<f32>(1.0, 0.0));
    let outside = vec2<f32>(-p.x - half.x, p.x - half.x) * end_scale;
    let joined = vec2<bool>(
        (rect.flags & RECT_JOINED_LEFT) != 0u,
        (rect.flags & RECT_JOINED_RIGHT) != 0u,
    );
    let ends = select(outside, select(vec2<f32>(-1.0e9), vec2<f32>(1.0e9), outside > vec2<f32>(0.0)), joined);
    return clamp(0.5 - max(distance, max(ends.x, ends.y)), 0.0, 1.0);
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

// A stroke field's 16-bit distance at `coord`, from 0 to 1. Coordinates past the glyph's slot,
// which `uv_rect` insets by half a texel, repeat its edge, which saturates.
fn stroke_texel(coord: vec2<i32>, uv_rect: vec4<f32>) -> f32 {
    let slot_min = vec2<i32>(floor(uv_rect.xy));
    let slot_max = vec2<i32>(floor(uv_rect.zw));
    let texel = textureLoad(atlas_texture, clamp(coord, slot_min, slot_max), 0);
    return dot(texel.rg, vec2<f32>(65280.0, 255.0) / 65535.0);
}

// A stroke field's distance at `texel`, filtered after decoding each texel, since a sampler may
// filter red and green with only their own 8 bits of precision.
fn stroke_field(texel: vec2<f32>, uv_rect: vec4<f32>) -> f32 {
    let base = floor(texel - vec2<f32>(0.5));
    let frac = texel - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let d00 = stroke_texel(p0, uv_rect);
    let d10 = stroke_texel(p0 + vec2<i32>(1, 0), uv_rect);
    let d01 = stroke_texel(p0 + vec2<i32>(0, 1), uv_rect);
    let d11 = stroke_texel(p0 + vec2<i32>(1, 1), uv_rect);
    return mix(mix(d00, d10, frac.x), mix(d01, d11, frac.x), frac.y);
}

fn median3(value: vec3<f32>) -> f32 {
    return max(min(value.r, value.g), min(max(value.r, value.g), value.b));
}

// Coverage of `glyph` at the untransformed position `local`, sampled from another instance's
// fragment whose texel moves `texel_width` per screen pixel and `lead_scale` per local pixel.
fn member_coverage(glyph: GlyphInstance, local: vec2<f32>, texel_width: vec2<f32>, lead_scale: vec2<f32>, filtered: bool) -> f32 {
    let texel = instance_texel(glyph, local);
    if ((glyph.mode & GLYPH_MODE_MASK) != 2u) {
        return select(
            raster_texel(vec2<i32>(floor(texel)), glyph.uv_rect),
            raster_bilinear(texel, glyph.uv_rect),
            filtered,
        ).a;
    }
    let inside = all(local >= glyph.rect.xy) && all(local <= glyph.rect.xy + glyph.rect.zw);
    let scale = (glyph.uv_rect.zw - glyph.uv_rect.xy) / glyph.rect.zw;
    let screen_px_range = max(
        0.5 * dot(vec2<f32>(globals.px_range), lead_scale / (texel_width * scale)),
        1.0,
    );
    let half_width = bitcast<f32>(paints[glyph.mode >> GLYPH_PAINT_SHIFT].paint.stroke);
    var coverage = 0.0;
    if (half_width > 0.0) {
        let tier = (glyph.flags >> GLYPH_RANGE_SHIFT) & GLYPH_RANGE_MASK;
        let range = 2.0 * globals.px_range * exp2(f32(tier));
        let reach = min(half_width * scale.x, 0.5 * (range - globals.px_range) - 1.5);
        let aa = 0.5 * globals.px_range / screen_px_range;
        let distance = abs(range * (stroke_field(texel, glyph.uv_rect) - 0.5));
        coverage = clamp((reach + aa - distance) / (2.0 * aa), 0.0, 1.0) * min(reach / aa, 1.0);
    } else {
        let field = textureSampleLevel(
            atlas_texture,
            atlas_sampler,
            texel / vec2<f32>(textureDimensions(atlas_texture)),
            0.0,
        );
        coverage = clamp(screen_px_range * (median3(field.rgb) - 0.5) + 0.5, 0.0, 1.0);
    }
    return select(0.0, coverage, inside);
}

// The premultiplied colour of `paint` at `in`'s pixel, whose untransformed position is `local`.
// A solid paint is the same everywhere, so it is read alone and never supersampled.
fn glyph_paint(in: VertexOut, paint: u32, local: vec2<f32>) -> vec4<f32> {
    if (paints[paint].paint.kind == 0u) {
        return premultiply(paints[paint].paint.colors[0]);
    }
    if (paints[paint].paint.paint_supersample != 0u) {
        return text_paint_aa(in, paint);
    }
    return text_paint_at(paint, local);
}

// The instances of a cluster that fades, each in its own paint, composited at full alpha so
// that they fade as one, at the untransformed position `local`. The cluster's lead instance
// draws them all, a faded glyph's stroke among them. `texel_width` is how far the lead's
// texel moves per screen pixel.
fn cluster_color(in: VertexOut, local: vec2<f32>, texel_width: vec2<f32>) -> vec4<f32> {
    let lead = glyphs[in.instance];
    let lead_scale = (lead.uv_rect.zw - lead.uv_rect.xy) / lead.rect.zw;
    let filtered = drawable_is_linear(style.transform) || in.transformed != 0u;
    var color = vec4<f32>(0.0);
    for (var member = 0u; member <= lead.flags >> GLYPH_CLUSTER_SHIFT; member = member + 1u) {
        let glyph = glyphs[in.instance + member];
        let paint = glyph_paint(in, glyph.mode >> GLYPH_PAINT_SHIFT, local);
        var source = vec4<f32>(0.0);
        if ((glyph.mode & GLYPH_MODE_MASK) == 1u) {
            let texel = instance_texel(glyph, local);
            source = select(
                raster_texel(vec2<i32>(floor(texel)), glyph.uv_rect),
                raster_bilinear(texel, glyph.uv_rect),
                filtered,
            ) * paint.a;
        } else {
            source = paint * member_coverage(glyph, local, texel_width, lead_scale, filtered);
        }
        color = source + color * (1.0 - source.a);
    }
    return color * partnered_alpha(in, local, texel_width, lead_scale, filtered);
}

// The composited alpha of the instances of the cluster led by `first` at `local`.
fn cluster_alpha(in: VertexOut, first: u32, local: vec2<f32>, texel_width: vec2<f32>, lead_scale: vec2<f32>, filtered: bool) -> f32 {
    var alpha = 0.0;
    for (var member = 0u; member <= glyphs[first].flags >> GLYPH_CLUSTER_SHIFT; member = member + 1u) {
        let glyph = glyphs[first + member];
        let source = member_coverage(glyph, local, texel_width, lead_scale, filtered)
            * glyph_paint(in, glyph.mode >> GLYPH_PAINT_SHIFT, local).a;
        alpha = source + alpha * (1.0 - source);
    }
    return alpha;
}

// The opacity to draw `in` at. A faded glyph's lower layer, its outside stroke or the fill under
// its centred stroke, is drawn as much as shows past its partner when the two are composited
// and then faded, so that compositing the partner over it at the glyph's opacity matches that.
fn partnered_alpha(in: VertexOut, local: vec2<f32>, texel_width: vec2<f32>, lead_scale: vec2<f32>, filtered: bool) -> f32 {
    let alpha = in.opacity;
    let lead = glyphs[in.instance];
    if ((lead.flags & GLYPH_PARTNER) == 0u || alpha >= 1.0) {
        return alpha;
    }
    let above = cluster_alpha(in, lead.partner, local, texel_width, lead_scale, filtered);
    return alpha * (1.0 - above) / max(1.0 - alpha * above, 1.0e-6);
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
    let fill = glyphs[in.instance].mode >> GLYPH_PAINT_SHIFT;
    let text_paint = glyph_paint(in, fill, local);
    if (in.mode == GLYPH_MODE_RECT) {
        return text_paint * (rect_coverage(glyphs[in.instance], in.texel) * in.opacity);
    }
    if ((glyphs[in.instance].flags >> GLYPH_CLUSTER_SHIFT) != 0u) {
        return cluster_color(in, local, texel_width);
    }
    // The atlas, the paint and the output are premultiplied. Opacity is applied once
    // the glyphs are composited, by the opacity pass below.
    if (in.mode == 1u) {
        return raster * (in.opacity * text_paint.a);
    }
    var coverage = raster.a;
    if (in.mode == 2u) {
        coverage = clamp(screen_px_range * (median3(field.rgb) - 0.5) + 0.5, 0.0, 1.0);
        let half_width = bitcast<f32>(paints[fill].paint.stroke);
        if (half_width > 0.0) {
            // A stroke's field holds the true distance to the outline in texels in 16 bits split
            // across red and green, saturating at half its range, which is twice `px_range`
            // times the power of two in the instance's flags. The stroke reaches at most that
            // less the widest antialiasing and a texel and a half, keeping in sync with
            // `field_reach` in `msdf.rs`, so texels that saturate stay uncovered. Antialiasing
            // follows the fill's and depends on the texel and pixel size alone, so crossing into
            // a wider range changes nothing else. A band narrower than its antialiasing fades
            // out with its width rather than keeping the half coverage at its centre.
            let tier = (glyphs[in.instance].flags >> GLYPH_RANGE_SHIFT) & GLYPH_RANGE_MASK;
            let range = 2.0 * globals.px_range * exp2(f32(tier));
            let texels_per_pixel = (in.uv_rect.z - in.uv_rect.x) / glyphs[in.instance].rect.z;
            let reach = min(half_width * texels_per_pixel, 0.5 * (range - globals.px_range) - 1.5);
            let aa = 0.5 * globals.px_range / screen_px_range;
            let encoded = stroke_field(in.texel, in.uv_rect);
            let distance = abs(range * (encoded - 0.5));
            coverage = clamp((reach + aa - distance) / (2.0 * aa), 0.0, 1.0) * min(reach / aa, 1.0);
        }
    }
    let lead_scale = (in.uv_rect.zw - in.uv_rect.xy) / glyphs[in.instance].rect.zw;
    let filtered = drawable_is_linear(style.transform) || in.transformed != 0u;
    let alpha = partnered_alpha(in, local, texel_width, lead_scale, filtered);
    return text_paint * (coverage * alpha);
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
