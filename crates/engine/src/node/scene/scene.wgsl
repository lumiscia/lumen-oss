// Draws objects of a 3D scene into a linear, premultiplied float target. One draw covers a run of
// objects from the object table, and each vertex of the draw belongs to the object whose range of
// the run's vertices it falls in. Vertices are pulled from storage: `indices` holds a mesh's
// vertex indices, one per vertex its objects draw.

const PI: f32 = 3.14159265;

const FLAG_UNLIT: u32 = 1u;
const FLAG_BLEND: u32 = 2u;
// Mirrored by its world matrix, so its outward faces wind clockwise.
const FLAG_MIRRORED: u32 = 4u;
// The bound raster texture colours the object.
const FLAG_RASTER: u32 = 8u;

const LIGHT_DIRECTIONAL: u32 = 0u;
const LIGHT_SPOT: u32 = 2u;

struct Globals {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    light_count: u32,
    // The camera's +Z axis in world space, towards the viewer.
    camera_back: vec3<f32>,
    orthographic: u32,
    ambient: vec3<f32>,
    // Brightness of the studio environment.
    environment: f32,
    fog_color: vec3<f32>,
    // Per world unit.
    fog_density: f32,
    // 0 none, 1 ACES, 2 AgX.
    tone_mapping: u32,
    // A multiplier.
    exposure: f32,
}

struct Object {
    model: mat4x4<f32>,
    // Takes normals to world space up to their length.
    normal: mat3x3<f32>,
    base_color: vec4<f32>,
    emissive: vec3<f32>,
    metallic: f32,
    roughness: f32,
    flags: u32,
    // Where the object's vertices start among all the table's.
    first_vertex: u32,
}

struct Vertex {
    position: array<f32, 3>,
    normal: array<f32, 3>,
    uv: array<f32, 2>,
}

// The rows of the object table a draw covers, and where their vertices start.
struct Run {
    first: u32,
    count: u32,
    base: u32,
    _pad: u32,
}

struct Light {
    position: vec3<f32>,
    kind: u32,
    // The direction light travels.
    direction: vec3<f32>,
    // 0 for unlimited.
    range: f32,
    radiance: vec3<f32>,
    // Cosines of a spot light's outer and inner angles.
    cos_outer: f32,
    cos_inner: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<storage, read> objects: array<Object>;
@group(0) @binding(2) var<storage, read> vertices: array<Vertex>;
@group(0) @binding(3) var<storage, read> indices: array<u32>;
@group(0) @binding(4) var<storage, read> lights: array<Light>;
@group(0) @binding(5) var<uniform> run: Run;
// A Lumen raster: premultiplied and sRGB-encoded.
@group(0) @binding(6) var raster_tex: texture_2d<f32>;
@group(0) @binding(7) var raster_sampler: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) object: u32,
    @location(3) uv: vec2<f32>,
}

// The row of the object table that draws vertex `slot` of the whole table.
fn object_of(slot: u32) -> u32 {
    var low = 0u;
    var high = run.count;
    while (high - low > 1u) {
        let middle = (low + high) / 2u;
        if (objects[run.first + middle].first_vertex <= slot) {
            low = middle;
        } else {
            high = middle;
        }
    }
    return run.first + low;
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    let slot = index + run.base;
    let oi = object_of(slot);
    let o = objects[oi];
    let vertex = vertices[indices[slot - o.first_vertex]];
    let position = vec3<f32>(vertex.position[0], vertex.position[1], vertex.position[2]);
    let normal = vec3<f32>(vertex.normal[0], vertex.normal[1], vertex.normal[2]);
    let world = o.model * vec4<f32>(position, 1.0);
    var out: VsOut;
    out.clip = globals.view_proj * world;
    out.world = world.xyz;
    out.normal = o.normal * normal;
    out.object = oi;
    out.uv = vec2<f32>(vertex.uv[0], vertex.uv[1]);
    return out;
}

fn distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn visibility_smith(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

fn fresnel_schlick(f0: vec3<f32>, v_dot_h: f32) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - v_dot_h, 5.0);
}

// Karis' analytic fit of the split-sum environment BRDF.
fn env_brdf(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// A disc of light around `center`, `radius` radians across, blurred by `blur` radians with its
// energy kept roughly the same.
fn softbox(direction: vec3<f32>, center: vec3<f32>, radius: f32, blur: f32) -> f32 {
    let angle = acos(clamp(dot(direction, normalize(center)), -1.0, 1.0));
    let spread = radius + blur * 0.75;
    let energy = (radius * radius) / (spread * spread);
    return (1.0 - smoothstep(radius - blur * 0.5, radius + blur, angle)) * energy;
}

// The ambient light's sky-to-ground gradient.
fn sky(direction: vec3<f32>) -> vec3<f32> {
    let sky = globals.ambient;
    return mix(sky * 0.35, sky, direction.y * 0.5 + 0.5);
}

// A studio of softboxes over a dim floor and ceiling. `blur` widens it for rough surfaces.
fn studio(direction: vec3<f32>, blur: f32) -> vec3<f32> {
    var room = vec3<f32>(mix(0.03, 0.2, smoothstep(-0.3, 0.8, direction.y)));
    room += vec3<f32>(3.0) * softbox(direction, vec3<f32>(0.0, 1.0, 0.35), 0.45, blur);
    room += vec3<f32>(2.0) * softbox(direction, vec3<f32>(-1.0, 0.25, 0.4), 0.3, blur);
    room += vec3<f32>(1.5) * softbox(direction, vec3<f32>(1.0, 0.35, -0.2), 0.25, blur);
    return room;
}

// Light a diffuse surface facing `n` receives from all around, over pi.
fn diffuse_environment(n: vec3<f32>) -> vec3<f32> {
    return sky(n) + studio(n, 1.2) * globals.environment;
}

// Light reflected towards the viewer along `direction` by a surface of `roughness`.
fn specular_environment(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    return sky(direction) + studio(direction, roughness * roughness * 1.5) * globals.environment;
}

// `v` at unit length, scaled first so that its squared length neither overflows nor underflows.
// Zero stays zero.
fn unit(v: vec3<f32>) -> vec3<f32> {
    let largest = max(max(abs(v.x), abs(v.y)), abs(v.z));
    if (largest == 0.0) {
        return v;
    }
    return normalize(v / largest);
}

// The direction from `world` towards the viewer.
fn view_direction(world: vec3<f32>) -> vec3<f32> {
    if (globals.orthographic != 0u) {
        return globals.camera_back;
    }
    return unit(globals.camera_position - world);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

// The raster at `uv` as linear colour with straight alpha.
fn raster_texel(uv: vec2<f32>) -> vec4<f32> {
    let texel = textureSampleLevel(raster_tex, raster_sampler, uv, 0.0);
    let straight = clamp(texel.rgb / max(texel.a, 1e-6), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(srgb_to_linear(straight), texel.a);
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let o = objects[in.object];
    let raster = (o.flags & FLAG_RASTER) != 0u;
    var texel = vec4<f32>(1.0);
    if (raster) {
        texel = raster_texel(in.uv);
    }
    var base = o.base_color * texel;
    let emissive = o.emissive * texel.rgb;
    if ((o.flags & FLAG_BLEND) == 0u) {
        base.a = 1.0;
    }
    if ((o.flags & FLAG_UNLIT) != 0u) {
        // A raster shown unlit keeps its colours as in 2D, whatever the exposure and tone mapping.
        let shown = base.rgb + emissive;
        return vec4<f32>(fogged(select(display(shown), shown, raster), in.world) * base.a, base.a);
    }

    let v = view_direction(in.world);
    // A normal the world matrix flattened away, on a face it left without area, faces the viewer.
    var n = v;
    let normal = unit(in.normal);
    if (any(normal != vec3<f32>(0.0))) {
        n = normal;
        let mirrored = (o.flags & FLAG_MIRRORED) != 0u;
        if (front == mirrored) {
            n = -n;
        }
    }
    let n_dot_v = max(dot(n, v), 1e-4);
    let metallic = clamp(o.metallic, 0.0, 1.0);
    let roughness = clamp(o.roughness, 0.03, 1.0);
    let alpha = roughness * roughness;
    let diffuse_color = base.rgb * (1.0 - metallic);
    let f0 = mix(vec3<f32>(0.04), base.rgb, metallic);

    var color = vec3<f32>(0.0);
    for (var i = 0u; i < globals.light_count; i += 1u) {
        let light = lights[i];
        var l: vec3<f32>;
        var radiance = light.radiance;
        if (light.kind == LIGHT_DIRECTIONAL) {
            l = -light.direction;
        } else {
            let to_light = light.position - in.world;
            let distance = length(to_light);
            l = to_light / max(distance, 1e-5);
            var falloff = 1.0 / max(distance * distance, 1e-4);
            if (light.range > 0.0) {
                let ratio = distance / light.range;
                let window = clamp(1.0 - ratio * ratio * ratio * ratio, 0.0, 1.0);
                falloff *= window * window;
            }
            radiance *= falloff;
            if (light.kind == LIGHT_SPOT) {
                radiance *= smoothstep(light.cos_outer, light.cos_inner, dot(-l, light.direction));
            }
        }
        let n_dot_l = dot(n, l);
        if (n_dot_l <= 0.0) {
            continue;
        }
        let h = normalize(l + v);
        let n_dot_h = max(dot(n, h), 0.0);
        let v_dot_h = max(dot(v, h), 0.0);
        let fresnel = fresnel_schlick(f0, v_dot_h);
        let specular = fresnel * distribution_ggx(n_dot_h, alpha) * visibility_smith(n_dot_v, n_dot_l, alpha);
        let diffuse = (vec3<f32>(1.0) - fresnel) * diffuse_color / PI;
        color += (diffuse + specular) * radiance * n_dot_l;
    }

    let reflected = reflect(-v, n);
    let blurred = normalize(mix(reflected, n, alpha * alpha));
    color += diffuse_color * diffuse_environment(n);
    color += specular_environment(blurred, roughness) * env_brdf(f0, roughness, n_dot_v);
    color += emissive;
    return vec4<f32>(fogged(display(color), in.world) * base.a, base.a);
}

// `color`, already in the displayed range, faded into the fog by the distance to `world`.
fn fogged(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let density = globals.fog_density;
    if (density <= 0.0) {
        return color;
    }
    let to_camera = globals.camera_position - world;
    let distance = select(length(to_camera), dot(to_camera, globals.camera_back), globals.orthographic != 0u);
    return mix(globals.fog_color, color, exp(-density * max(distance, 0.0)));
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2
        + 0.1191 * x - 0.00232;
}

// Minimal AgX, returned in linear light.
fn agx(color: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        vec3<f32>(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3<f32>(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3<f32>(0.0792237451477643, 0.0791661274605434, 0.879142973793104),
    );
    let outset = mat3x3<f32>(
        vec3<f32>(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3<f32>(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3<f32>(-0.0990297440797205, -0.0989611768448433, 1.15107367264116),
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = inset * color;
    v = clamp(log2(max(v, vec3<f32>(1e-10))), vec3<f32>(min_ev), vec3<f32>(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);
    v = outset * agx_contrast(v);
    return pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2));
}

// Scene light brought into the 0..1 range shown: exposed, then tone mapped. Done per surface,
// before fog and blending.
fn display(color: vec3<f32>) -> vec3<f32> {
    var exposed = max(color, vec3<f32>(0.0)) * globals.exposure;
    if (globals.tone_mapping == 1u) {
        exposed = aces(exposed);
    } else if (globals.tone_mapping == 2u) {
        exposed = agx(exposed);
    }
    return clamp(exposed, vec3<f32>(0.0), vec3<f32>(1.0));
}
