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
    orthogonality: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> jobs: array<Job>;
@group(0) @binding(3) var<storage, read> segments: array<Segment>;

fn job_for_pixel(pixel: u32) -> u32 {
    var low = 0u;
    var high = globals.job_count;
    loop {
        if (low >= high) {
            break;
        }
        let middle = low + (high - low) / 2u;
        let job = jobs[middle];
        if (pixel < job.pixel_range.x) {
            high = middle;
        } else if (pixel >= job.pixel_range.x + job.pixel_range.y) {
            low = middle + 1u;
        } else {
            return middle;
        }
    }
    return globals.job_count;
}

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
    let orthogonality = abs(cross2(ab, closest - p))
        / sqrt(max(dot(ab, ab) * dot(closest - p, closest - p), 0.000001));
    return SignedDistance(
        cross2_nonzero_sign(cross2(p - a, ab)) * distance,
        raw_t,
        orthogonality,
    );
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
    let orthogonality = abs(cross2(tangent, closest - p))
        / sqrt(max(dot(tangent, tangent) * dot(closest - p, closest - p), 0.000001));
    return SignedDistance(
        cross2_nonzero_sign(cross2(p - closest, tangent)) * distance,
        t,
        orthogonality,
    );
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

fn approximate_distance(segment: Segment, p: vec2<f32>) -> SignedDistance {
    if (segment.kind == 0u) {
        return signed_line_distance(segment.p0, segment.p1, p);
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
    return signed_curve_distance(segment, p, best_t);
}

fn line_ray_winding(a: vec2<f32>, b: vec2<f32>, p: vec2<f32>) -> i32 {
    if ((a.y > p.y) == (b.y > p.y)) {
        return 0i;
    }
    let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
    if (x <= p.x) {
        return 0i;
    }
    return select(-1i, 1i, b.y > a.y);
}

fn segment_ray_winding(segment: Segment, p: vec2<f32>) -> i32 {
    if (segment.kind == 0u) {
        return line_ray_winding(segment.p0, segment.p1, p);
    }

    // Flattening gives quadratic and cubic contours the same half-open crossing
    // rule as lines. Solving curve roots directly is faster in isolation, but
    // roots at horizontal extrema can be counted twice or omitted and invert an
    // entire atlas row. This work only occurs when a glyph enters the cache.
    let steps = select(32u, 48u, segment.kind == 2u);
    var winding = 0i;
    var previous = segment.p0;
    for (var i = 1u; i <= steps; i += 1u) {
        let next = point_on_segment(segment, f32(i) / f32(steps));
        winding += line_ray_winding(previous, next, p);
        previous = next;
    }
    return winding;
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

    // Jobs have contiguous, sorted pixel ranges. Resolving the owner here avoids a
    // four-byte CPU/GPU lookup entry for every generated atlas pixel.
    let job_index = job_for_pixel(dirty_pixel);
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
    var nearest_true_distance = vec3<f32>(1.0e20);
    var nearest_pseudo_distance = vec3<f32>(1.0e20);
    var nearest_orthogonality = vec3<f32>(0.0);
    var nearest_shape_distance = 1.0e20;
    var nearest_shape_pseudo_distance = 1.0e20;
    var nearest_shape_orthogonality = 0.0;
    var winding = 0i;
    for (var i = 0u; i < job.segment_range.y; i = i + 1u) {
        let segment = segments[job.segment_range.x + i];
        let distance = approximate_distance(segment, p);
        let pseudo_distance = endpoint_pseudo_distance(segment, distance, p);
        let shape_delta = abs(distance.distance) - abs(nearest_shape_distance);
        if (shape_delta < -0.0001
            || (abs(shape_delta) <= 0.0001
                && distance.orthogonality > nearest_shape_orthogonality)) {
            nearest_shape_distance = distance.distance;
            nearest_shape_pseudo_distance = pseudo_distance;
            nearest_shape_orthogonality = distance.orthogonality;
        }
        winding += segment_ray_winding(segment, p);
        let red_delta = abs(distance.distance) - abs(nearest_true_distance.r);
        if ((segment.channels & 1u) != 0u
            && (red_delta < -0.0001
                || (abs(red_delta) <= 0.0001 && distance.orthogonality > nearest_orthogonality.r))) {
            nearest_true_distance.r = distance.distance;
            nearest_pseudo_distance.r = pseudo_distance;
            nearest_orthogonality.r = distance.orthogonality;
        }
        let green_delta = abs(distance.distance) - abs(nearest_true_distance.g);
        if ((segment.channels & 2u) != 0u
            && (green_delta < -0.0001
                || (abs(green_delta) <= 0.0001 && distance.orthogonality > nearest_orthogonality.g))) {
            nearest_true_distance.g = distance.distance;
            nearest_pseudo_distance.g = pseudo_distance;
            nearest_orthogonality.g = distance.orthogonality;
        }
        let blue_delta = abs(distance.distance) - abs(nearest_true_distance.b);
        if ((segment.channels & 4u) != 0u
            && (blue_delta < -0.0001
                || (abs(blue_delta) <= 0.0001 && distance.orthogonality > nearest_orthogonality.b))) {
            nearest_true_distance.b = distance.distance;
            nearest_pseudo_distance.b = pseudo_distance;
            nearest_orthogonality.b = distance.orthogonality;
        }
    }
    let shape_sign = select(-1.0, 1.0, winding != 0i);

    textureStore(
        atlas,
        pixel,
        vec4<f32>(
            encode_distance(abs(nearest_pseudo_distance.r) * shape_sign, job.px_range),
            encode_distance(abs(nearest_pseudo_distance.g) * shape_sign, job.px_range),
            encode_distance(abs(nearest_pseudo_distance.b) * shape_sign, job.px_range),
            encode_distance(
                abs(nearest_shape_pseudo_distance) * shape_sign,
                job.px_range,
            ),
        ),
    );
}
