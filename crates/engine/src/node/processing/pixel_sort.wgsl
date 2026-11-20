struct PixelSortParams {
    // The length of a line in device pixels, the canvas position of its first texel, the segment
    // length, and the segment length rounded up to a power of two.
    line: vec4<i32>,
    // The first segment of the line, the segments a workgroup sorts, and 0, 0.
    segments: vec4<i32>,
    // Whether the sort is vertical and whether it is reversed, each 0 or 1, then the key.
    mode: vec4<u32>,
    // The lowest and highest key that is sorted.
    band: vec4<f32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: PixelSortParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

const SORT_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const SLOTS: u32 = 1024u;
const LANES: u32 = 256u;
const PER_LANE: u32 = 4u;
const DEAD: u32 = 0xffffffffu;
const IN_BAND: u32 = 0x10000u;

// One slot per texel of a workgroup's segments, each segment padded to a power of two.
// `keys[slot]` holds the sort key in `x`, with the run's first slot in its upper half, and the
// texel's place in its segment in `y`. `starts[slot]` is the first slot of its run.
var<workgroup> keys: array<vec2<u32>, 1024>;
var<workgroup> starts: array<u32, 1024>;

fn texel(line: i32, along: i32) -> vec2<i32> {
    if (params.mode.x == 1u) {
        return vec2<i32>(line, along);
    }
    return vec2<i32>(along, line);
}

// The key of the straight sRGB-encoded colour `rgb`, 0 to 1.
fn key_of(rgb: vec3<f32>) -> f32 {
    if (params.mode.z == 0u) {
        return dot(rgb, SORT_LUMA);
    }
    let high = max(rgb.r, max(rgb.g, rgb.b));
    let span = high - min(rgb.r, min(rgb.g, rgb.b));
    if (params.mode.z == 2u) {
        return select(0.0, span / high, high > 0.0);
    }
    if (span <= 0.0) {
        return 0.0;
    }
    var sector: f32;
    if (high == rgb.r) {
        sector = (rgb.g - rgb.b) / span;
    } else if (high == rgb.g) {
        sector = 2.0 + (rgb.b - rgb.r) / span;
    } else {
        sector = 4.0 + (rgb.r - rgb.g) / span;
    }
    return fract(sector / 6.0 + 1.0);
}

fn precedes(a: vec2<u32>, b: vec2<u32>) -> bool {
    return a.x < b.x || (a.x == b.x && a.y < b.y);
}

// Sorts the runs of in-band texels in each segment of the workgroup. Texels out of the band
// are runs of their own, so they keep their place: sorting by the first slot of the run, then the
// key, then the place in the segment, moves each texel only within its run.
@compute @workgroup_size(256, 1, 1)
fn cs_main(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let line = i32(group.y);
    let line_length = params.line.x;
    let origin = params.line.y;
    let window = params.line.z;
    let padded = params.line.w;
    let per_group = params.segments.y;
    let first = params.segments.x + i32(group.x) * per_group;
    let reverse = params.mode.y == 1u;

    for (var j = 0u; j < PER_LANE; j++) {
        let slot = lane + j * LANES;
        let segment = i32(slot) / padded;
        let place = i32(slot) % padded;
        let along = (first + segment) * window + place - origin;
        var entry = vec2<u32>(DEAD, u32(place));
        if (segment < per_group && place < window) {
            // A slot of the window past either end of the line is out of the band, so it keeps
            // its place and the texels after it keep theirs.
            entry = vec2<u32>(0u, u32(place));
        }
        if (segment < per_group && place < window && along >= 0 && along < line_length) {
            let color = textureLoad(input_tex, texel(line, along), 0);
            if (color.a > 0.0) {
                let rgb = clamp(lumen_linear_to_srgb(unpremultiply(color).rgb), vec3<f32>(0.0), vec3<f32>(1.0));
                let key = key_of(rgb);
                if (key >= params.band.x && key <= params.band.y) {
                    let level = u32(round(clamp(key, 0.0, 1.0) * 65535.0));
                    entry = vec2<u32>(select(level, 65535u - level, reverse), u32(place) | IN_BAND);
                }
            }
        }
        keys[slot] = entry;
    }
    workgroupBarrier();

    // The first slot of each texel's run: its own, unless the texel before it is in the same run,
    // found as the largest first slot among the texels up to it.
    for (var j = 0u; j < PER_LANE; j++) {
        let slot = lane + j * LANES;
        let place = u32(i32(slot) % padded);
        let continues = place > 0u && (keys[slot].y & IN_BAND) != 0u && (keys[slot - 1u].y & IN_BAND) != 0u;
        starts[slot] = select(place, 0u, continues);
    }
    workgroupBarrier();
    for (var reach = 1; reach < padded; reach *= 2) {
        var next: array<u32, 4>;
        for (var j = 0u; j < PER_LANE; j++) {
            let slot = lane + j * LANES;
            next[j] = starts[slot];
            if (i32(slot) % padded >= reach) {
                next[j] = max(next[j], starts[slot - u32(reach)]);
            }
        }
        workgroupBarrier();
        for (var j = 0u; j < PER_LANE; j++) {
            starts[lane + j * LANES] = next[j];
        }
        workgroupBarrier();
    }
    for (var j = 0u; j < PER_LANE; j++) {
        let slot = lane + j * LANES;
        let entry = keys[slot];
        if (entry.x != DEAD) {
            let in_band = (entry.y & IN_BAND) != 0u;
            let run = select(entry.y, starts[slot], in_band);
            keys[slot] = vec2<u32>((run << 16u) | select(0u, entry.x, in_band), entry.y & 0xffffu);
        }
    }
    workgroupBarrier();

    // Bitonic sort of each segment's padded keys.
    let half_size = u32(padded) / 2u;
    let pairs = u32(per_group) * half_size;
    for (var width = 2u; width <= u32(padded); width *= 2u) {
        for (var stride = width / 2u; stride > 0u; stride /= 2u) {
            for (var pair = lane; pair < pairs; pair += LANES) {
                let segment = pair / half_size;
                let within = pair % half_size;
                let low = (within / stride) * 2u * stride + within % stride;
                let a = segment * u32(padded) + low;
                let b = a + stride;
                let ascending = (low & width) == 0u;
                let key_a = keys[a];
                let key_b = keys[b];
                if (precedes(key_b, key_a) == ascending) {
                    keys[a] = key_b;
                    keys[b] = key_a;
                }
            }
            workgroupBarrier();
        }
    }

    for (var j = 0u; j < PER_LANE; j++) {
        let slot = lane + j * LANES;
        let segment = i32(slot) / padded;
        let place = i32(slot) % padded;
        let along = (first + segment) * window + place - origin;
        if (segment < per_group && place < window && along >= 0 && along < line_length) {
            let source_at = (first + segment) * window + i32(keys[slot].y) - origin;
            textureStore(output_tex, texel(line, along), textureLoad(input_tex, texel(line, source_at), 0));
        }
    }
}
