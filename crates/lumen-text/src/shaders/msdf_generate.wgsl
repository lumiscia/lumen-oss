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

struct SignedDistance {
    distance: f32,
    param: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> jobs: array<Job>;
@group(0) @binding(3) var<storage, read> segments: array<Segment>;
@group(0) @binding(4) var<storage, read> pixel_jobs: array<u32>;

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
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

fn cross2_nonzero_sign(value: f32) -> f32 {
    return select(-1.0, 1.0, value >= 0.0);
}

fn signed_line_distance(a: vec2<f32>, b: vec2<f32>, p: vec2<f32>) -> SignedDistance {
    let ab = b - a;
    let denom = max(dot(ab, ab), 0.000001);
    let raw_t = dot(p - a, ab) / denom;
    let t = clamp(raw_t, 0.0, 1.0);
    let closest = a + ab * t;
    let distance = length(p - closest);
    return SignedDistance(cross2_nonzero_sign(cross2(p - a, ab)) * distance, raw_t);
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

fn signed_curve_distance(segment: Segment, p: vec2<f32>, t0: f32) -> SignedDistance {
    var t = clamp(t0, 0.0, 1.0);
    for (var i = 0u; i < 4u; i = i + 1u) {
        let q = point_on_segment(segment, t);
        let tangent = segment_tangent(segment, t);
        let speed2 = max(dot(tangent, tangent), 0.000001);
        t = clamp(t - dot(q - p, tangent) / speed2, 0.0, 1.0);
    }
    let closest = point_on_segment(segment, t);
    let tangent = segment_tangent(segment, t);
    let distance = length(p - closest);
    return SignedDistance(cross2_nonzero_sign(cross2(p - closest, tangent)) * distance, t);
}

fn endpoint_pseudo_distance(segment: Segment, signed_distance: SignedDistance, p: vec2<f32>) -> f32 {
    var distance = signed_distance.distance;
    if (signed_distance.param <= 0.0) {
        let dir = normalize(segment_tangent(segment, 0.0));
        let aq = p - point_on_segment(segment, 0.0);
        if (dot(aq, dir) < 0.0) {
            let pseudo_distance = cross2(aq, dir);
            if (abs(pseudo_distance) <= abs(distance)) {
                distance = pseudo_distance;
            }
        }
    } else if (signed_distance.param >= 1.0) {
        let dir = normalize(segment_tangent(segment, 1.0));
        let bq = p - point_on_segment(segment, 1.0);
        if (dot(bq, dir) > 0.0) {
            let pseudo_distance = cross2(bq, dir);
            if (abs(pseudo_distance) <= abs(distance)) {
                distance = pseudo_distance;
            }
        }
    }
    return distance;
}

fn approximate_distance(segment: Segment, p: vec2<f32>) -> f32 {
    if (segment.kind == 0u) {
        return abs(endpoint_pseudo_distance(segment, signed_line_distance(segment.p0, segment.p1, p), p));
    }

    var best = 1.0e20;
    var best_t = 0.0;
    var prev = segment.p0;
    let steps = select(24u, 32u, segment.kind == 2u);
    for (var i = 1u; i <= steps; i = i + 1u) {
        let t = f32(i) / f32(steps);
        let next = point_on_segment(segment, t);
        let distance = abs(signed_line_distance(prev, next, p).distance);
        if (distance < best) {
            best = distance;
            best_t = (f32(i) - 0.5) / f32(steps);
        }
        prev = next;
    }
    return abs(endpoint_pseudo_distance(segment, signed_curve_distance(segment, p, best_t), p));
}

fn line_ray_crosses(a: vec2<f32>, b: vec2<f32>, p: vec2<f32>) -> u32 {
    if ((a.y > p.y) == (b.y > p.y)) {
        return 0u;
    }
    let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
    return select(0u, 1u, x > p.x);
}

fn quadratic_ray_crosses(segment: Segment, p: vec2<f32>) -> u32 {
    let a = segment.p0.y - 2.0 * segment.p1.y + segment.p2.y;
    let b = 2.0 * (segment.p1.y - segment.p0.y);
    let c = segment.p0.y - p.y;
    var crossings = 0u;

    if (abs(a) < 0.000001) {
        if (abs(b) < 0.000001) {
            return 0u;
        }
        let t = -c / b;
        if (t >= 0.0 && t < 1.0) {
            let q = point_on_segment(segment, t);
            crossings = crossings + select(0u, 1u, q.x > p.x);
        }
        return crossings;
    }

    let discriminant = b * b - 4.0 * a * c;
    if (discriminant < 0.0) {
        return 0u;
    }
    let root = sqrt(discriminant);
    let t0 = (-b - root) / (2.0 * a);
    let t1 = (-b + root) / (2.0 * a);
    if (t0 >= 0.0 && t0 < 1.0) {
        let q = point_on_segment(segment, t0);
        crossings = crossings + select(0u, 1u, q.x > p.x);
    }
    if (abs(t1 - t0) > 0.00001 && t1 >= 0.0 && t1 < 1.0) {
        let q = point_on_segment(segment, t1);
        crossings = crossings + select(0u, 1u, q.x > p.x);
    }
    return crossings;
}

fn segment_ray_crossings(segment: Segment, p: vec2<f32>) -> u32 {
    if (segment.kind == 0u) {
        return line_ray_crosses(segment.p0, segment.p1, p);
    }
    if (segment.kind == 1u) {
        return quadratic_ray_crosses(segment, p);
    }

    var crossings = 0u;
    var prev = segment.p0;
    let steps = select(64u, 96u, segment.kind == 2u);
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
    var nearest_distance = 1.0e20;
    var crossings = 0u;
    for (var i = 0u; i < job.segment_range.y; i = i + 1u) {
        let segment = segments[job.segment_range.x + i];
        let distance = approximate_distance(segment, p);
        if (distance < nearest_distance) {
            nearest_distance = distance;
        }
        crossings = crossings + segment_ray_crossings(segment, p);
    }
    let sign = select(-1.0, 1.0, (crossings & 1u) == 1u);
    let shape_distance = nearest_distance * sign;

    textureStore(
        atlas,
        pixel,
        vec4<f32>(
            encode_distance(shape_distance, job.px_range),
            encode_distance(shape_distance, job.px_range),
            encode_distance(shape_distance, job.px_range),
            1.0,
        ),
    );
}
