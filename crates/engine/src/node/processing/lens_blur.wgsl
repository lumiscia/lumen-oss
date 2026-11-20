// Lens blur: a gather with flat weights over a disc or polygon whose radius, the circle of
// confusion (CoC), is set per pixel.
//
// `prepare_main` copies the source over the output's region into level 0, with each pixel's CoC
// and depth. `reduce_main` halves it, level by level: colour and depth averaged, the smallest
// CoC of each block kept, so a block across a sharp edge doesn't spread it, and where in the
// block its light is, so a highlight's bokeh stays where the highlight is at any level. A circle
// wider than `GATHER_TEXELS` texels is gathered on the level that halves it below that, so every
// gather reads a bounded number of texels, and it crossfades into the next level over its last
// tenth so the switch doesn't band.
// Level 0 has textures of its own, and odd and even levels above it are stacked down one texture
// each, so a reduction reads one texture while it writes the other.
//
// With depth, each pixel blends two gathers. The background gathers, within the pixel's own
// CoC, the samples at its depth or behind it, each limited to the smaller of its CoC and the
// pixel's, and normalises them. The foreground gathers the samples nearer than the pixel whose
// own CoC reaches it, within the radius, each weighted by its share of its circle's area, so a
// sharp pixel shows how much of it a blurred foreground covers, and goes over the background. Without depth every
// CoC is the radius, so only the background runs and gives the plain average over the circle.
//
// A level texel is a block of `2^level` canvas pixels, and blocks are aligned to the canvas.
// Pixels past the canvas read the nearest edge pixel. Levels above 0 hold one block more on each
// side, which past the canvas holds those repeated edge pixels, as every block further out would,
// so reads past it read it. Blocks outside the region a level covers read as transparent, keeping
// the nearest covered block's CoC and depth. A level's second
// texture holds the CoC, the depth, and the centre of the block's light as an offset from the
// block's centre, in blocks.

struct LensBlurParams {
    // Radius in device pixels, focus, focus range and aperture.
    values: vec4<f32>,
    // Blades (0 for a circle), rotation in radians, and 1 with a depth input.
    shape: vec4<f32>,
    // Canvas pixel of the output's first texel in `xy`, the source's in `zw`.
    origins: vec4<i32>,
    // Canvas pixel of the depth's first texel in `xy`, and its display size in `zw`.
    depth: vec4<i32>,
    // Levels built, and the level a reduction writes in `z`.
    levels: vec4<i32>,
    // Per level: where it starts in its texture in `xy`, its first block on the canvas in `zw`.
    level_origin: array<vec4<i32>, 15>,
    // Per level: its size in texels in `xy`, the canvas's size in its blocks in `zw`.
    level_size: array<vec4<i32>, 15>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: LensBlurParams;
@group(0) @binding(3) var in_color: texture_2d<f32>;
@group(0) @binding(4) var in_coc: texture_2d<f32>;
@group(0) @binding(5) var out_color: texture_storage_2d<rgba16float, write>;
@group(0) @binding(6) var out_coc: texture_storage_2d<rgba16float, write>;
@group(0) @binding(7) var level0_color: texture_2d<f32>;
@group(0) @binding(8) var level0_coc: texture_2d<f32>;
@group(0) @binding(9) var odd_color: texture_2d<f32>;
@group(0) @binding(10) var odd_coc: texture_2d<f32>;
@group(0) @binding(11) var even_color: texture_2d<f32>;
@group(0) @binding(12) var even_coc: texture_2d<f32>;
@group(0) @binding(13) var output_tex: texture_storage_2d<rgba16float, write>;

const GATHER_TEXELS: f32 = 6.0;
const BLEND_FROM: f32 = 5.4;
const PI: f32 = 3.14159265;

struct Texel {
    color: vec4<f32>,
    coc: f32,
    depth: f32,
    // Where the texel's light is centred, from its block's centre, in blocks.
    offset: vec2<f32>,
}

fn has_depth() -> bool {
    return params.shape.z > 0.5;
}

// The texel of canvas block `block` of `level`, stored in `color_tex` and `coc_tex`.
fn load_level(color_tex: texture_2d<f32>, coc_tex: texture_2d<f32>, level: i32, block: vec2<i32>) -> Texel {
    let origin = params.level_origin[level];
    let size = params.level_size[level];
    let pad = select(1, 0, level == 0);
    let local = clamp(block, vec2<i32>(-pad), size.zw - vec2<i32>(1 - pad)) - origin.zw;
    let inside = all(local >= vec2<i32>(0)) && all(local < size.xy);
    let at = clamp(local, vec2<i32>(0), size.xy - vec2<i32>(1)) + origin.xy;
    let data = textureLoad(coc_tex, at, 0);
    var color = vec4<f32>(0.0);
    if (inside) {
        color = textureLoad(color_tex, at, 0);
    }
    return Texel(color, data.x, data.y, data.zw);
}

@compute @workgroup_size(8, 8, 1)
fn prepare_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(params.level_size[0].xy);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = vec2<i32>(id.xy) + params.origins.xy;
    let color = sample_or_transparent(source_tex, pixel, params.origins.zw);
    var coc = params.values.x;
    var depth = 0.0;
    if (has_depth()) {
        let at = clamp(pixel, vec2<i32>(0), params.depth.zw - vec2<i32>(1));
        depth = sample_or_transparent(depth_tex, at, params.depth.xy).r;
        let away = abs(depth - params.values.y) - params.values.z;
        coc = coc * clamp(params.values.w * away, 0.0, 1.0);
    }
    textureStore(out_color, vec2<i32>(id.xy), color);
    textureStore(out_coc, vec2<i32>(id.xy), vec4<f32>(coc, depth, 0.0, 0.0));
}

@compute @workgroup_size(8, 8, 1)
fn reduce_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let level = params.levels.z;
    let size = vec2<u32>(params.level_size[level].xy);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let block = vec2<i32>(id.xy) + params.level_origin[level].zw;
    var color = vec4<f32>(0.0);
    var coc = 1e30;
    var depth = 0.0;
    var centre = vec2<f32>(0.0);
    var light = 0.0;
    for (var y = 0; y < 2; y = y + 1) {
        for (var x = 0; x < 2; x = x + 1) {
            let texel = load_level(in_color, in_coc, level - 1, block * 2 + vec2<i32>(x, y));
            color = color + texel.color;
            coc = min(coc, texel.coc);
            depth = depth + texel.depth;
            // A texel's light in this block, from its centre, weighted by how bright it is.
            let weight = dot(texel.color, vec4<f32>(1.0)) + 1e-6;
            centre = centre + weight * (vec2<f32>(f32(x), f32(y)) - vec2<f32>(0.5) + texel.offset) * 0.5;
            light = light + weight;
        }
    }
    let at = vec2<i32>(id.xy) + params.level_origin[level].xy;
    textureStore(out_color, at, color * 0.25);
    textureStore(out_coc, at, vec4<f32>(coc, depth * 0.25, centre / light));
}

fn level_texel(level: i32, block: vec2<i32>) -> Texel {
    if (level == 0) {
        return load_level(level0_color, level0_coc, level, block);
    }
    if ((level & 1) == 1) {
        return load_level(odd_color, odd_coc, level, block);
    }
    return load_level(even_color, even_coc, level, block);
}

// How far `offset` is from the centre in units where the kernel's edge is its radius: its length
// for a circle, and for a polygon its length along the nearest side's normal, scaled so the
// corners are at the radius.
fn shape_distance(offset: vec2<f32>) -> f32 {
    let distance = length(offset);
    let blades = params.shape.x;
    if (blades < 3.0 || distance == 0.0) {
        return distance;
    }
    let side = 2.0 * PI / blades;
    // The first side's normal is half a side clockwise from the corner pointing up.
    let normal = params.shape.y - 0.5 * PI + 0.5 * side;
    let angle = atan2(offset.y, offset.x) - normal;
    let from_normal = angle - side * round(angle / side);
    return distance * cos(from_normal) / cos(0.5 * side);
}

// The kernel's area for a radius of `radius` texels.
fn kernel_area(radius: f32) -> f32 {
    let blades = params.shape.x;
    if (blades < 3.0) {
        return PI * radius * radius;
    }
    return 0.5 * blades * radius * radius * sin(2.0 * PI / blades);
}

// The level a circle of `radius` device pixels is gathered on, and how far it has crossfaded into
// the next one.
fn gather_level(radius: f32) -> vec2<f32> {
    let coarsest = f32(params.levels.x - 2);
    let level = clamp(ceil(log2(max(radius, 1e-6) / GATHER_TEXELS)), 0.0, coarsest);
    let texels = radius / exp2(level);
    return vec2<f32>(level, smoothstep(BLEND_FROM, GATHER_TEXELS, texels));
}

struct Gathered {
    sum: vec4<f32>,
    weight: f32,
}

// Gathers `level` within `radius` device pixels of canvas position `at`. The background takes
// the samples at `depth` or behind it, each reaching as far as the smaller of its CoC and `coc`;
// the foreground takes the samples nearer than `depth`, each reaching as far as its own CoC.
// Weights are what a sample's circle gives each texel it covers, its edge softened over a texel.
// A sample sits where its light is centred, which may be anywhere in its block.
fn gather(level: i32, at: vec2<f32>, radius: f32, coc: f32, depth: f32, foreground: bool) -> Gathered {
    let block = exp2(f32(level));
    let reach = radius + 0.5 * block;
    let low = vec2<i32>(floor((at - vec2<f32>(reach)) / block));
    let high = vec2<i32>(floor((at + vec2<f32>(reach)) / block));
    let within = reach + 0.7072 * block;
    let depth_tested = has_depth();
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    for (var y = low.y; y <= high.y; y = y + 1) {
        for (var x = low.x; x <= high.x; x = x + 1) {
            let centre = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * block - at;
            if (length(centre) > within) {
                continue;
            }
            let texel = level_texel(level, vec2<i32>(x, y));
            let offset = centre + texel.offset * block;
            let nearer = depth_tested && texel.depth < depth;
            if (nearer != foreground) {
                continue;
            }
            var texels: f32;
            if (foreground) {
                texels = texel.coc / block;
                // A circle inside its own texel covers no other.
                if (texels < 0.5) {
                    continue;
                }
            } else {
                // A circle covers at least its own texel.
                texels = max(min(texel.coc, coc) / block, 0.5);
            }
            let cover = clamp(texels - shape_distance(offset) / block + 0.5, 0.0, 1.0);
            if (cover <= 0.0) {
                continue;
            }
            let share = cover / kernel_area(texels);
            sum = sum + share * texel.color;
            weight = weight + share;
        }
    }
    return Gathered(sum, weight);
}

fn normalized(gathered: Gathered) -> vec4<f32> {
    if (gathered.weight <= 0.0) {
        return vec4<f32>(0.0);
    }
    return gathered.sum / gathered.weight;
}

// The samples at `depth` or behind it within `coc` of `at`, averaged by their weights.
fn background(at: vec2<f32>, coc: f32, depth: f32, own: vec4<f32>) -> vec4<f32> {
    let level = gather_level(coc);
    let near = gather(i32(level.x), at, coc, coc, depth, false);
    if (near.weight <= 0.0) {
        return own;
    }
    var color = normalized(near);
    if (level.y > 0.0) {
        let far = gather(i32(level.x) + 1, at, coc, coc, depth, false);
        if (far.weight > 0.0) {
            color = mix(color, normalized(far), level.y);
        }
    }
    return color;
}

// What the samples nearer than `depth` whose CoC reaches `at` cover it with, premultiplied, and
// no more than all of it.
fn foreground(at: vec2<f32>, radius: f32, depth: f32) -> vec4<f32> {
    let level = gather_level(radius);
    var gathered = gather(i32(level.x), at, radius, radius, depth, true);
    if (level.y > 0.0) {
        let far = gather(i32(level.x) + 1, at, radius, radius, depth, true);
        gathered = Gathered(mix(gathered.sum, far.sum, level.y), mix(gathered.weight, far.weight, level.y));
    }
    return gathered.sum / max(gathered.weight, 1.0);
}

@compute @workgroup_size(8, 8, 1)
fn resolve_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = vec2<i32>(id.xy) + params.origins.xy;
    let at = vec2<f32>(pixel) + vec2<f32>(0.5);
    let own = level_texel(0, pixel);
    var color = background(at, min(own.coc, params.values.x), own.depth, own.color);
    if (has_depth()) {
        let front = foreground(at, params.values.x, own.depth);
        color = front + color * (1.0 - front.a);
    }
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(color.rgb, clamp(color.a, 0.0, 1.0)));
}
