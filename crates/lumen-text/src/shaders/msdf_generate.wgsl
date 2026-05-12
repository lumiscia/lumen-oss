struct Globals {
    atlas_size: vec2<u32>,
    job_count: u32,
    dirty_pixel_count: u32,
    _padding0: u32,
    _padding1: u32,
}

struct Job {
    atlas_rect: vec4<u32>,
    segment_range: vec2<u32>,
    pixel_range: vec2<u32>,
    px_range: f32,
    _padding0: u32,
    _padding1: u32,
    _padding2: u32,
}

struct Segment {
    p0: vec2<f32>,
    p1: vec2<f32>,
    p2: vec2<f32>,
    p3: vec2<f32>,
    kind: u32,
    channels: u32,
    _padding0: u32,
    _padding1: u32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<storage, read> jobs: array<Job>;
@group(0) @binding(3) var<storage, read> segments: array<Segment>;
@group(0) @binding(4) var<storage, read> pixel_jobs: array<u32>;

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

fn median3(value: vec3<f32>) -> f32 {
    return max(min(value.r, value.g), min(max(value.r, value.g), value.b));
}

fn point_on_segment(segment: Segment, t: f32) -> vec2<f32> {
    let u = 1.0 - t;
    if (segment.kind == 0u) {
        return mix(segment.p0, segment.p1, t);
    }
    if (segment.kind == 1u) {
        return u * u * segment.p0 + 2.0 * u * t * segment.p1 + t * t * segment.p2;
    }
    return u * u * u * segment.p0
        + 3.0 * u * u * t * segment.p1
        + 3.0 * u * t * t * segment.p2
        + t * t * t * segment.p3;
}

fn line_distance(a: vec2<f32>, b: vec2<f32>, p: vec2<f32>) -> f32 {
    let ab = b - a;
    let denom = max(dot(ab, ab), 0.000001);
    let t = clamp(dot(p - a, ab) / denom, 0.0, 1.0);
    return length(p - (a + ab * t));
}

fn segment_tangent(segment: Segment, t: f32) -> vec2<f32> {
    if (segment.kind == 0u) {
        return segment.p1 - segment.p0;
    }
    if (segment.kind == 1u) {
        return 2.0 * mix(segment.p1 - segment.p0, segment.p2 - segment.p1, t);
    }
    let u = 1.0 - t;
    return 3.0 * u * u * (segment.p1 - segment.p0)
        + 6.0 * u * t * (segment.p2 - segment.p1)
        + 3.0 * t * t * (segment.p3 - segment.p2);
}

fn refine_curve_distance(segment: Segment, p: vec2<f32>, t0: f32) -> f32 {
    var t = clamp(t0, 0.0, 1.0);
    for (var i = 0u; i < 4u; i = i + 1u) {
        let q = point_on_segment(segment, t);
        let tangent = segment_tangent(segment, t);
        let speed2 = max(dot(tangent, tangent), 0.000001);
        t = clamp(t - dot(q - p, tangent) / speed2, 0.0, 1.0);
    }
    return length(point_on_segment(segment, t) - p);
}

fn line_ray_crosses(a: vec2<f32>, b: vec2<f32>, p: vec2<f32>) -> u32 {
    if ((a.y > p.y) == (b.y > p.y)) {
        return 0u;
    }
    let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
    return select(0u, 1u, x > p.x);
}

fn approximate_distance(segment: Segment, p: vec2<f32>) -> f32 {
    if (segment.kind == 0u) {
        return line_distance(segment.p0, segment.p1, p);
    }

    var best = 1.0e20;
    var best_t = 0.0;
    var prev = segment.p0;
    let steps = select(16u, 24u, segment.kind == 2u);
    for (var i = 1u; i <= steps; i = i + 1u) {
        let t = f32(i) / f32(steps);
        let next = point_on_segment(segment, t);
        let distance = line_distance(prev, next, p);
        if (distance < best) {
            best = distance;
            best_t = (f32(i) - 0.5) / f32(steps);
        }
        prev = next;
    }
    return refine_curve_distance(segment, p, best_t);
}

fn segment_ray_crossings(segment: Segment, p: vec2<f32>) -> u32 {
    if (segment.kind == 0u) {
        return line_ray_crosses(segment.p0, segment.p1, p);
    }

    var crossings = 0u;
    var prev = segment.p0;
    let steps = select(16u, 24u, segment.kind == 2u);
    for (var i = 1u; i <= steps; i = i + 1u) {
        let t = f32(i) / f32(steps);
        let next = point_on_segment(segment, t);
        crossings = crossings + line_ray_crosses(prev, next, p);
        prev = next;
    }
    return crossings;
}

fn encode_distance(distance: f32, px_range: f32) -> f32 {
    return clamp(distance / px_range + 0.5, 0.0, 1.0);
}

@compute @workgroup_size(64, 1, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dirty_pixel = id.x;
    if (dirty_pixel >= globals.dirty_pixel_count) {
        return;
    }

    let job_index = pixel_jobs[dirty_pixel];
    if (job_index >= globals.job_count) {
        return;
    }
    let job = jobs[job_index];

    let local_pixel = dirty_pixel - job.pixel_range.x;
    let local = vec2<u32>(local_pixel % job.atlas_rect.z, local_pixel / job.atlas_rect.z);
    let pixel = job.atlas_rect.xy + local;
    if (pixel.x >= globals.atlas_size.x || pixel.y >= globals.atlas_size.y) {
        return;
    }

    let p = vec2<f32>(local) + vec2<f32>(0.5);
    var d = vec3<f32>(1.0e20);
    var crossings = 0u;
    for (var i = 0u; i < job.segment_range.y; i = i + 1u) {
        let segment = segments[job.segment_range.x + i];
        let distance = approximate_distance(segment, p);
        crossings = crossings + segment_ray_crossings(segment, p);
        if ((segment.channels & 1u) != 0u && distance < d.r) {
            d.r = distance;
        }
        if ((segment.channels & 2u) != 0u && distance < d.g) {
            d.g = distance;
        }
        if ((segment.channels & 4u) != 0u && distance < d.b) {
            d.b = distance;
        }
    }

    let fallback = median3(d);
    if (d.r == 1.0e20) {
        d.r = fallback;
    }
    if (d.g == 1.0e20) {
        d.g = fallback;
    }
    if (d.b == 1.0e20) {
        d.b = fallback;
    }
    let sign = select(-1.0, 1.0, (crossings & 1u) == 1u);
    textureStore(
        atlas,
        pixel,
        vec4<f32>(
            encode_distance(d.r * sign, job.px_range),
            encode_distance(d.g * sign, job.px_range),
            encode_distance(d.b * sign, job.px_range),
            1.0,
        ),
    );
}
