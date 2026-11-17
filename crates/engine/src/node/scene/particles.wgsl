// Places a particles_3d's particles: one invocation writes one particle's row of the instance
// buffer the scene shader draws copies from. A particle is a slot whose birth, emit point,
// direction, size and turn come from a stream of random numbers seeded by the node's seed, the
// slot and the generation, the count of times it has been born again.

const PI: f32 = 3.14159265;
// Particles turn their +Y to the normal they are emitted along, in `Params::flags`.
const ALIGN: u32 = 1u;
// Particles never age, as all are born at once to live forever where they are emitted.
const STILL: u32 = 2u;

struct Params {
    count: u32,
    seed: u32,
    // Objects of the surface, or 0 to emit from the origin along +Y.
    parts: u32,
    flags: u32,
    // Seconds into the composition.
    time: f32,
    rate: f32,
    lifetime: f32,
    speed: f32,
    gravity: vec3<f32>,
    cos_spread: f32,
    scale: vec2<f32>,
    scale_over_life: vec2<f32>,
    // Radians.
    jitter: f32,
}

// One object of the surface: the rows of its affine world matrix, where its triangles are in
// `triangles`, its share of the surface's area added to those of the objects before it, and the
// sign of its matrix's determinant, which turns its faces' normals back when it mirrors them.
struct Part {
    rows: array<vec4<f32>, 3>,
    first: u32,
    count: u32,
    weight: f32,
    turn: f32,
}

// A triangle's corners in its mesh, the first's `w` holding its share of its mesh's area added to
// those of the triangles before it.
struct Triangle {
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
}

// Keep in sync with `Instance` in `scene.wgsl`.
struct Instance {
    rows: array<vec4<f32>, 3>,
    tint: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> parts: array<Part>;
@group(0) @binding(2) var<storage, read> triangles: array<Triangle>;
@group(0) @binding(3) var<storage, read_write> instances: array<Instance>;

var<private> stream: u32;

// The PCG hash.
fn hash(value: u32) -> u32 {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// The stream's next number, from 0 up to but not including 1.
fn random() -> f32 {
    stream = hash(stream);
    return f32(stream >> 8u) / 16777216.0;
}

// The first part whose running share is above `at`.
fn pick_part(at: f32) -> u32 {
    var low = 0u;
    var high = params.parts - 1u;
    while (low < high) {
        let middle = (low + high) / 2u;
        if (parts[middle].weight > at) {
            high = middle;
        } else {
            low = middle + 1u;
        }
    }
    return low;
}

// The first of `count` triangles from `first` whose running share is above `at`.
fn pick_triangle(first: u32, count: u32, at: f32) -> u32 {
    var low = first;
    var high = first + count - 1u;
    while (low < high) {
        let middle = (low + high) / 2u;
        if (triangles[middle].a.w > at) {
            high = middle;
        } else {
            low = middle + 1u;
        }
    }
    return low;
}

// `p` placed by the affine matrix whose rows are `rows`, as a point when `w` is 1 and a direction
// when it is 0.
fn place(rows: array<vec4<f32>, 3>, p: vec3<f32>, w: f32) -> vec3<f32> {
    let q = vec4<f32>(p, w);
    return vec3<f32>(dot(rows[0], q), dot(rows[1], q), dot(rows[2], q));
}

// A turn of `angle` radians about the unit `axis`.
fn turn_about(axis: vec3<f32>, angle: f32) -> mat3x3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    let k = 1.0 - c;
    let a = axis;
    return mat3x3<f32>(
        vec3<f32>(c + a.x * a.x * k, a.y * a.x * k + a.z * s, a.z * a.x * k - a.y * s),
        vec3<f32>(a.x * a.y * k - a.z * s, c + a.y * a.y * k, a.z * a.y * k + a.x * s),
        vec3<f32>(a.x * a.z * k + a.y * s, a.y * a.z * k - a.x * s, c + a.z * a.z * k),
    );
}

// The least turn taking +Y to the unit `normal`.
fn turn_up_to(normal: vec3<f32>) -> mat3x3<f32> {
    let axis = vec3<f32>(normal.z, 0.0, -normal.x);
    let s = length(axis);
    if (s < 1e-6) {
        if (normal.y > 0.0) {
            return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
        }
        return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, -1.0, 0.0), vec3<f32>(0.0, 0.0, -1.0));
    }
    return turn_about(axis / s, atan2(s, normal.y));
}

// A direction picked evenly over the sphere.
fn random_direction() -> vec3<f32> {
    let z = random() * 2.0 - 1.0;
    let around = random() * 2.0 * PI;
    let r = sqrt(max(0.0, 1.0 - z * z));
    return vec3<f32>(r * cos(around), r * sin(around), z);
}

// A particle that draws nothing.
fn hidden() -> Instance {
    var out: Instance;
    out.rows = array<vec4<f32>, 3>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    out.tint = vec4<f32>(1.0);
    return out;
}

@compute @workgroup_size(64, 1, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let slot = id.x;
    if (slot >= params.count) {
        return;
    }
    var age = 0.0;
    var generation = 0u;
    if ((params.flags & STILL) == 0u) {
        var born = 0.0;
        if (params.rate > 0.0) {
            let first = f32(slot) / params.rate;
            let period = f32(params.count) / params.rate;
            let since = params.time - first;
            if (since < 0.0) {
                instances[slot] = hidden();
                return;
            }
            let times = floor(since / period);
            generation = u32(times);
            born = first + times * period;
        } else if (params.time < 0.0) {
            instances[slot] = hidden();
            return;
        }
        age = max(params.time - born, 0.0);
        if (params.lifetime > 0.0 && age >= params.lifetime) {
            instances[slot] = hidden();
            return;
        }
    }
    stream = hash(params.seed ^ hash(slot ^ hash(generation)));

    var origin = vec3<f32>(0.0);
    var normal = vec3<f32>(0.0, 1.0, 0.0);
    if (params.parts > 0u) {
        let part = parts[pick_part(random())];
        let triangle = triangles[pick_triangle(part.first, part.count, random())];
        var u = random();
        var v = random();
        if (u + v > 1.0) {
            u = 1.0 - u;
            v = 1.0 - v;
        }
        let a = triangle.a.xyz;
        let ab = triangle.b.xyz - a;
        let ac = triangle.c.xyz - a;
        origin = place(part.rows, a + ab * u + ac * v, 1.0);
        let face = cross(place(part.rows, ab, 0.0), place(part.rows, ac, 0.0)) * part.turn;
        let area = length(face);
        if (area > 0.0) {
            normal = face / area;
        }
    }

    // Evenly over the cap of the sphere within the spread of the normal.
    let cos_lean = mix(1.0, params.cos_spread, random());
    let sin_lean = sqrt(max(0.0, 1.0 - cos_lean * cos_lean));
    let around = random() * 2.0 * PI;
    var side = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(normal.x) > 0.9) {
        side = vec3<f32>(0.0, 1.0, 0.0);
    }
    let tangent = normalize(cross(side, normal));
    let bitangent = cross(normal, tangent);
    let direction = normal * cos_lean + (tangent * cos(around) + bitangent * sin(around)) * sin_lean;
    let position = origin + direction * (params.speed * age) + params.gravity * (0.5 * age * age);

    let axis = random_direction();
    var turn = turn_about(axis, random() * params.jitter);
    if ((params.flags & ALIGN) != 0u) {
        turn = turn_up_to(normal) * turn;
    }
    var size = mix(params.scale.x, params.scale.y, random());
    if (params.lifetime > 0.0) {
        size *= mix(params.scale_over_life.x, params.scale_over_life.y, clamp(age / params.lifetime, 0.0, 1.0));
    } else {
        size *= params.scale_over_life.x;
    }
    let placed = turn * max(size, 0.0);

    var out: Instance;
    out.rows = array<vec4<f32>, 3>(
        vec4<f32>(placed[0].x, placed[1].x, placed[2].x, position.x),
        vec4<f32>(placed[0].y, placed[1].y, placed[2].y, position.y),
        vec4<f32>(placed[0].z, placed[1].z, placed[2].z, position.z),
    );
    out.tint = vec4<f32>(1.0);
    instances[slot] = out;
}
