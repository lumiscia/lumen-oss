// Draws objects of a 3D scene into a linear, premultiplied float target. One draw covers a run of
// objects from the object table, and each vertex of the draw belongs to the object whose range of
// the run's vertices it falls in. Vertices are pulled from storage: `indices` holds a mesh's
// vertex indices, from each object's first, one per vertex the object draws. A vertex is morphed
// by its morph targets, then skinned.

const PI: f32 = 3.14159265;

const FLAG_UNLIT: u32 = 1u;
const FLAG_BLEND: u32 = 2u;
// Mirrored by its world matrix, so its outward faces wind clockwise.
const FLAG_MIRRORED: u32 = 4u;
// The bound raster texture colours the object.
const FLAG_RASTER: u32 = 8u;
// Drawn only where its alpha reaches `alpha_cutoff`, and opaque there.
const FLAG_MASK: u32 = 16u;
// Its vertices move by their joints, which take them to the space `model` places.
const FLAG_SKINNED: u32 = 32u;
// Each face is lit by its own normal rather than the vertices'.
const FLAG_FLAT_NORMALS: u32 = 64u;
const FLAG_BASE_COLOR_MAP: u32 = 128u;
const FLAG_METALLIC_ROUGHNESS_MAP: u32 = 256u;
const FLAG_NORMAL_MAP: u32 = 512u;
const FLAG_EMISSIVE_MAP: u32 = 1024u;
const FLAG_OCCLUSION_MAP: u32 = 2048u;
const FLAG_RECEIVE_SHADOWS: u32 = 4096u;
// Shows only the shadows on it, as black whose alpha is the fraction of light they take away.
const FLAG_SHADOW_CATCHER: u32 = 8192u;
// The base colour is multiplied by the vertices' colours.
const FLAG_VERTEX_COLORS: u32 = 16384u;
// Maps read their texture coordinates through the object's rows of `map_uvs`.
const FLAG_MAP_UVS: u32 = 32768u;
// No vertex extras, in `Object::extras`.
const NO_EXTRAS: u32 = 0xffffffffu;
// Words of one morph target displacement: a position's, then a normal's.
const MORPH_DELTA_WORDS: u32 = 6u;
// The map slots, in the order of an object's rows of `map_uvs`.
const SLOT_BASE_COLOR: u32 = 0u;
const SLOT_METALLIC_ROUGHNESS: u32 = 1u;
const SLOT_NORMAL: u32 = 2u;
const SLOT_EMISSIVE: u32 = 3u;
const SLOT_OCCLUSION: u32 = 4u;

const LIGHT_DIRECTIONAL: u32 = 0u;
const LIGHT_POINT: u32 = 1u;
const LIGHT_SPOT: u32 = 2u;

// Taps of the shadow map a shadow's edge is filtered from, each filtered between four texels.
const SHADOW_TAPS: u32 = 32u;
// The widest the taps spread, in texels; `MAX_FILTER_TEXELS` in `lumen_3d`.
const MAX_SHADOW_RADIUS: f32 = 16.0;
// Cells of the environment's split-sum table along each axis; `BRDF_TABLE_SIZE` in `lumen_3d`.
const BRDF_TABLE_SIZE: u32 = 32u;

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
    // Samples per output pixel along each axis.
    supersample: u32,
    // The camera's +X and +Y axes in world space, which normals are output in with `camera_back`.
    camera_right: vec3<f32>,
    // The distances along the camera's view that depth output maps to 0 and 1.
    depth_near: f32,
    camera_up: vec3<f32>,
    depth_far: f32,
}

// An equirectangular environment map, whose centre looks along -Z with +X to its right.
struct Environment {
    // Irradiance over pi as spherical-harmonic coefficients, in the map's own frame.
    irradiance: array<vec4<f32>, 9>,
    // Clip space to world space, for the directions behind each pixel.
    clip_to_world: mat4x4<f32>,
    // The cosine and sine of the map's turn about +Y.
    turn: vec2<f32>,
    // The last reflection level, which is for roughness 1.
    roughest: f32,
    // The roughness the background is blurred to.
    background_blur: f32,
    // 1 when a map lights the scene in place of the studio.
    map: u32,
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
    // Where the object's indices start in `indices`.
    first_index: u32,
    // Where a skinned object's joints start in `joints`.
    first_joint: u32,
    alpha_cutoff: f32,
    normal_scale: f32,
    occlusion_strength: f32,
    // Where the mesh's vertex extras start in `vertex_data`, four words a vertex, or `NO_EXTRAS`.
    extras: u32,
    // Where the object's morph target displacements start in `vertex_data`, vertex by vertex
    // from `morph_vertex`, each vertex's targets in order.
    first_delta: u32,
    morph_vertex: u32,
    // 0 without morph targets.
    morph_targets: u32,
    // Where its morph target weights start in `weights`.
    first_weight: u32,
    // Where its maps' five rows start in `map_uvs`.
    first_map_uv: u32,
}

struct Vertex {
    position: array<f32, 3>,
    normal: array<f32, 3>,
    uv: array<f32, 2>,
}

// One map's texture coordinates: the rows of an affine transform of `(u, v, 1)`, with the set it
// reads, 0 or 1, last in the first.
struct MapUv {
    first: vec4<f32>,
    second: vec4<f32>,
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
    // The light's first shadow view, six of them for a point light, or -1 without shadows.
    shadow: i32,
    // World width over which shadow edges blur.
    shadow_softness: f32,
    // How far from the camera along its view a directional light's shadows reach.
    shadow_distance: f32,
}

// One view a light casts shadows from, a tile of the shadow atlas.
struct ShadowView {
    // World to the view's clip space, whose X and Y span its tile from -1 to 1.
    view_proj: mat4x4<f32>,
    // A world point's depth from the light is `dot(depth.xyz, p) + depth.w`, 1 at the view's far
    // end.
    depth: vec4<f32>,
    // The tile as fractions of the atlas: left, top, right and bottom.
    tile: vec4<f32>,
    // World width of a texel; for a perspective view, at depth 1 and in proportion to depth.
    texel: f32,
    perspective: u32,
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
@group(0) @binding(8) var<storage, read> joints: array<mat4x4<f32>>;
// What a model's vertices hold beyond `vertices`: a skinned mesh's four joints and their weights
// from the start, four words a vertex packing two 16-bit values to a word; then its vertex
// extras and morph target displacements, where each object says.
@group(0) @binding(9) var<storage, read> vertex_data: array<u32>;
// A material's maps. Base colour and emissive are sRGB textures, read as linear light.
@group(0) @binding(10) var base_color_map: texture_2d<f32>;
@group(0) @binding(11) var metallic_roughness_map: texture_2d<f32>;
@group(0) @binding(12) var normal_map: texture_2d<f32>;
@group(0) @binding(13) var emissive_map: texture_2d<f32>;
@group(0) @binding(14) var occlusion_map: texture_2d<f32>;
@group(0) @binding(15) var base_color_sampler: sampler;
@group(0) @binding(16) var metallic_roughness_sampler: sampler;
@group(0) @binding(17) var normal_sampler: sampler;
@group(0) @binding(18) var emissive_sampler: sampler;
@group(0) @binding(19) var occlusion_sampler: sampler;
@group(0) @binding(20) var<storage, read> shadow_views: array<ShadowView>;
// Each view's depth over the light's reach, 1 where nothing casts a shadow.
@group(0) @binding(21) var shadow_map: texture_depth_2d;
@group(0) @binding(22) var shadow_sampler: sampler_comparison;
@group(0) @binding(23) var<uniform> environment: Environment;
// The map's reflections, mip level `i` for a roughness of `i / roughest`.
@group(0) @binding(24) var environment_map: texture_2d<f32>;
// The map at its decoded size, for the background.
@group(0) @binding(25) var environment_background: texture_2d<f32>;
// Wraps around horizontally and clamps vertically.
@group(0) @binding(26) var environment_sampler: sampler;
// The split sum's scale and bias of `f0`: by n·v at cell centres along each row, and by roughness
// from 0 to 1 down the rows.
@group(0) @binding(27) var<storage, read> environment_brdf: array<vec2<f32>>;
// Morphed objects' morph target weights.
@group(0) @binding(28) var<storage, read> weights: array<f32>;
@group(0) @binding(29) var<storage, read> map_uvs: array<MapUv>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) object: u32,
    @location(3) uv: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    // Linear, with straight alpha.
    @location(5) color: vec4<f32>,
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

// The weighted blend of the joints of skinned `vertex`, or none when its weights are all zero.
fn skinning(vertex: u32, first_joint: u32) -> mat4x4<f32> {
    let at = vertex * 4u;
    let ids = vec4<u32>(
        vertex_data[at] & 0xffffu,
        vertex_data[at] >> 16u,
        vertex_data[at + 1u] & 0xffffu,
        vertex_data[at + 1u] >> 16u,
    );
    let weights = vec4<f32>(unpack2x16unorm(vertex_data[at + 2u]), unpack2x16unorm(vertex_data[at + 3u]));
    let total = weights.x + weights.y + weights.z + weights.w;
    if (total <= 0.0) {
        return mat4x4<f32>(
            vec4<f32>(1.0, 0.0, 0.0, 0.0),
            vec4<f32>(0.0, 1.0, 0.0, 0.0),
            vec4<f32>(0.0, 0.0, 1.0, 0.0),
            vec4<f32>(0.0, 0.0, 0.0, 1.0),
        );
    }
    var blended = joints[first_joint + ids.x] * (weights.x / total);
    for (var i = 1u; i < 4u; i += 1u) {
        blended += joints[first_joint + ids[i]] * (weights[i] / total);
    }
    return blended;
}

// What turns normals as `m` turns positions, up to their length: its cofactor matrix, negated
// where `m` mirrors so normals keep facing out.
fn normal_turn(m: mat4x4<f32>) -> mat3x3<f32> {
    let x = m[0].xyz;
    let y = m[1].xyz;
    let z = m[2].xyz;
    let sign = select(1.0, -1.0, dot(x, cross(y, z)) < 0.0);
    return mat3x3<f32>(cross(y, z) * sign, cross(z, x) * sign, cross(x, y) * sign);
}

// A vertex of the run being drawn, posed in world space.
struct Posed {
    object: u32,
    world: vec4<f32>,
    // In world space up to its length.
    normal: vec3<f32>,
    uv: vec2<f32>,
    uv1: vec2<f32>,
    color: vec4<f32>,
}

fn data_vec3(at: u32) -> vec3<f32> {
    return vec3<f32>(bitcast<f32>(vertex_data[at]), bitcast<f32>(vertex_data[at + 1u]), bitcast<f32>(vertex_data[at + 2u]));
}

fn posed(index: u32) -> Posed {
    let slot = index + run.base;
    let oi = object_of(slot);
    let o = objects[oi];
    let vertex_index = indices[o.first_index + slot - o.first_vertex];
    let vertex = vertices[vertex_index];
    var position = vec4<f32>(vertex.position[0], vertex.position[1], vertex.position[2], 1.0);
    var normal = vec3<f32>(vertex.normal[0], vertex.normal[1], vertex.normal[2]);
    if (o.morph_targets > 0u) {
        let unmorphed = normal;
        let first = o.first_delta + (vertex_index - o.morph_vertex) * o.morph_targets * MORPH_DELTA_WORDS;
        for (var i = 0u; i < o.morph_targets; i += 1u) {
            let weight = weights[o.first_weight + i];
            if (weight != 0.0) {
                let at = first + i * MORPH_DELTA_WORDS;
                position += vec4<f32>(data_vec3(at) * weight, 0.0);
                normal += data_vec3(at + 3u) * weight;
            }
        }
        // A direction again, or the unmorphed one where the targets cancel it out.
        let largest = max(max(abs(normal.x), abs(normal.y)), abs(normal.z));
        normal = select(unmorphed, unit(normal), largest > 1e-6);
    }
    if ((o.flags & FLAG_SKINNED) != 0u) {
        let skin = skinning(vertex_index, o.first_joint);
        position = skin * position;
        normal = normal_turn(skin) * normal;
    }
    var out: Posed;
    out.object = oi;
    out.world = o.model * position;
    // Unit length at each vertex, so that its length doesn't weigh how it is interpolated.
    out.normal = unit(o.normal * normal);
    out.uv = vec2<f32>(vertex.uv[0], vertex.uv[1]);
    out.uv1 = out.uv;
    out.color = vec4<f32>(1.0);
    if (o.extras != NO_EXTRAS) {
        let at = o.extras + vertex_index * 4u;
        out.uv1 = vec2<f32>(bitcast<f32>(vertex_data[at]), bitcast<f32>(vertex_data[at + 1u]));
        if ((o.flags & FLAG_VERTEX_COLORS) != 0u) {
            out.color = vec4<f32>(unpack2x16unorm(vertex_data[at + 2u]), unpack2x16unorm(vertex_data[at + 3u]));
        }
    }
    return out;
}

// Both sets of a fragment's texture coordinates, with their change over a sample.
struct Uvs {
    uv0: vec2<f32>,
    dx0: vec2<f32>,
    dy0: vec2<f32>,
    uv1: vec2<f32>,
    dx1: vec2<f32>,
    dy1: vec2<f32>,
}

// Where a map reads, and its change over a sample.
struct MapCoords {
    uv: vec2<f32>,
    dx: vec2<f32>,
    dy: vec2<f32>,
}

// Where `o`'s map in `slot` reads: the set its row names, moved by its transform.
fn map_coords(o: Object, slot: u32, uvs: Uvs) -> MapCoords {
    var out = MapCoords(uvs.uv0, uvs.dx0, uvs.dy0);
    if (!has(o.flags, FLAG_MAP_UVS)) {
        return out;
    }
    let row = map_uvs[o.first_map_uv + slot];
    if (row.first.w > 0.5) {
        out = MapCoords(uvs.uv1, uvs.dx1, uvs.dy1);
    }
    let a = row.first.xyz;
    let b = row.second.xyz;
    return MapCoords(
        vec2<f32>(dot(a, vec3<f32>(out.uv, 1.0)), dot(b, vec3<f32>(out.uv, 1.0))),
        vec2<f32>(dot(a.xy, out.dx), dot(b.xy, out.dx)),
        vec2<f32>(dot(a.xy, out.dy), dot(b.xy, out.dy)),
    );
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    let vertex = posed(index);
    var out: VsOut;
    out.clip = globals.view_proj * vertex.world;
    out.world = vertex.world.xyz;
    out.normal = vertex.normal;
    out.object = vertex.object;
    out.uv = vertex.uv;
    out.uv1 = vertex.uv1;
    out.color = vertex.color;
    return out;
}

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    // X, Y and W of the vertex's place in its view's own clip space, to keep fragments in its tile.
    @location(0) view_clip: vec3<f32>,
    @location(1) depth: f32,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) object: u32,
    @location(4) uv1: vec2<f32>,
    @location(5) alpha: f32,
}

// The vertex seen from the shadow view of the instance being drawn, squeezed into its tile.
@vertex
fn vs_shadow(@builtin(vertex_index) index: u32, @builtin(instance_index) view_index: u32) -> ShadowOut {
    let vertex = posed(index);
    let view = shadow_views[view_index];
    let clip = view.view_proj * vertex.world;
    let scale = view.tile.zw - view.tile.xy;
    let offset = vec2<f32>(view.tile.x + view.tile.z - 1.0, 1.0 - view.tile.y - view.tile.w);
    var out: ShadowOut;
    out.clip = vec4<f32>(clip.xy * scale + offset * clip.w, clip.z, clip.w);
    out.view_clip = clip.xyw;
    out.depth = dot(view.depth.xyz, vertex.world.xyz) + view.depth.w;
    out.uv = vertex.uv;
    out.object = vertex.object;
    out.uv1 = vertex.uv1;
    out.alpha = vertex.color.a;
    return out;
}

// Whether `o` is solid where it reads `uv` and `uv1` with vertex alpha `alpha`, which is where it
// blocks light and is drawn into depth and normal output: everywhere for an opaque surface, where
// a masked one's alpha reaches its cutoff and where a blended one's reaches half.
fn solid(o: Object, uv: vec2<f32>, uv1: vec2<f32>, vertex_alpha: f32) -> bool {
    if (!has(o.flags, FLAG_MASK) && !has(o.flags, FLAG_BLEND)) {
        return true;
    }
    var alpha = o.base_color.a * vertex_alpha;
    if (has(o.flags, FLAG_RASTER)) {
        alpha *= textureSampleLevel(raster_tex, raster_sampler, uv, 0.0).a;
    }
    if (has(o.flags, FLAG_BASE_COLOR_MAP)) {
        let still = vec2<f32>(0.0);
        let at = map_coords(o, SLOT_BASE_COLOR, Uvs(uv, still, still, uv1, still, still)).uv;
        alpha *= textureSampleLevel(base_color_map, base_color_sampler, at, 0.0).a;
    }
    return alpha >= select(0.5, o.alpha_cutoff, has(o.flags, FLAG_MASK));
}

// Writes depth from the view's light within its tile, where the surface is solid.
@fragment
fn fs_shadow(in: ShadowOut) -> @builtin(frag_depth) f32 {
    if (any(abs(in.view_clip.xy) > vec2<f32>(in.view_clip.z))) {
        discard;
    }
    if (!solid(objects[in.object], in.uv, in.uv1, in.alpha)) {
        discard;
    }
    return clamp(in.depth, 0.0, 1.0);
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
    if (environment.map != 0u) {
        return sky(n) + map_irradiance(n) * globals.environment;
    }
    return sky(n) + studio(n, 1.2) * globals.environment;
}

// Light reflected towards the viewer along `direction` by a surface of `roughness`.
fn specular_environment(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    if (environment.map != 0u) {
        return sky(direction) + map_reflection(direction, roughness) * globals.environment;
    }
    return sky(direction) + studio(direction, roughness * roughness * 1.5) * globals.environment;
}

// Where the environment map shows the world direction `d`, turned into the map's frame.
fn map_uv(d: vec3<f32>) -> vec2<f32> {
    let c = environment.turn.x;
    let s = environment.turn.y;
    let m = unit(vec3<f32>(c * d.x - s * d.z, d.y, s * d.x + c * d.z));
    return vec2<f32>(0.5 + atan2(m.x, -m.z) / (2.0 * PI), acos(clamp(m.y, -1.0, 1.0)) / PI);
}

// The map's light reflected along `direction` by a surface of `roughness`.
fn map_reflection(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    let level = clamp(roughness, 0.0, 1.0) * environment.roughest;
    return textureSampleLevel(environment_map, environment_sampler, map_uv(direction), level).rgb;
}

// The map's irradiance over pi on a surface facing `n`, from its harmonics; `sh_basis` in
// `lumen_3d`.
fn map_irradiance(n: vec3<f32>) -> vec3<f32> {
    let c = environment.turn.x;
    let s = environment.turn.y;
    let d = vec3<f32>(c * n.x - s * n.z, n.y, s * n.x + c * n.z);
    let sh = environment.irradiance;
    var e = sh[0].rgb * 0.282095;
    e += (sh[1].rgb * d.y + sh[2].rgb * d.z + sh[3].rgb * d.x) * 0.488603;
    e += (sh[4].rgb * (d.x * d.y) + sh[5].rgb * (d.y * d.z) + sh[7].rgb * (d.x * d.z)) * 1.092548;
    e += sh[6].rgb * (0.315392 * (3.0 * d.z * d.z - 1.0));
    e += sh[8].rgb * (0.546274 * (d.x * d.x - d.y * d.y));
    return max(e, vec3<f32>(0.0));
}

// The split sum's scale and bias of `f0` at `n_dot_v` and `roughness`, interpolated in its table.
fn environment_split_sum(n_dot_v: f32, roughness: f32) -> vec2<f32> {
    let last = f32(BRDF_TABLE_SIZE - 1u);
    let x = clamp(n_dot_v * f32(BRDF_TABLE_SIZE) - 0.5, 0.0, last);
    let y = clamp(roughness, 0.0, 1.0) * last;
    let x0 = u32(x);
    let y0 = u32(y);
    let x1 = min(x0 + 1u, BRDF_TABLE_SIZE - 1u);
    let y1 = min(y0 + 1u, BRDF_TABLE_SIZE - 1u);
    let top = mix(environment_brdf[y0 * BRDF_TABLE_SIZE + x0], environment_brdf[y0 * BRDF_TABLE_SIZE + x1], x - f32(x0));
    let bottom = mix(environment_brdf[y1 * BRDF_TABLE_SIZE + x0], environment_brdf[y1 * BRDF_TABLE_SIZE + x1], x - f32(x0));
    return mix(top, bottom, y - f32(y0));
}

// How much of the reflected light from all around an occlusion of `occlusion` leaves (Lagarde).
fn specular_occlusion(n_dot_v: f32, occlusion: f32, roughness: f32) -> f32 {
    return clamp(pow(n_dot_v + occlusion, exp2(-16.0 * roughness - 1.0)) - 1.0 + occlusion, 0.0, 1.0);
}

struct BackgroundOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

// A triangle covering the target.
@vertex
fn vs_background(@builtin(vertex_index) index: u32) -> BackgroundOut {
    let ndc = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0;
    var out: BackgroundOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

// The environment map seen behind the pixel, displayed as surfaces are and unfogged, blurred
// from the sharp map into its reflection levels.
@fragment
fn fs_background(in: BackgroundOut) -> @location(0) vec4<f32> {
    let near = environment.clip_to_world * vec4<f32>(in.ndc, 0.0, 1.0);
    let far = environment.clip_to_world * vec4<f32>(in.ndc, 1.0, 1.0);
    let uv = map_uv(far.xyz / far.w - near.xyz / near.w);
    let level = clamp(environment.background_blur, 0.0, 1.0) * environment.roughest;
    var radiance = textureSampleLevel(environment_map, environment_sampler, uv, level).rgb;
    if (level < 1.0) {
        let sharp = textureSampleLevel(environment_background, environment_sampler, uv, 0.0).rgb;
        radiance = mix(sharp, radiance, level);
    }
    return vec4<f32>(display(radiance * globals.environment), 1.0);
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

// Where a sample of the raster belongs: at `uv`, moved to the centre of the output pixel the
// sample is part of, so that the samples averaged into a pixel all read the same place and a
// raster shown at its own size or larger isn't filtered twice. A raster shown smaller keeps each
// sample's own place, since averaging distinct samples is what filters it, and moving them
// together would shrink that filter. `dx` and `dy` are the change in `uv` over a sample, and
// `position` the sample's place in the supersampled target.
fn pixel_centre_uv(position: vec2<f32>, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec2<f32> {
    let scale = f32(max(globals.supersample, 1u));
    let to_centre = (floor(position / scale) + vec2<f32>(0.5)) * scale - position;
    let size = vec2<f32>(textureDimensions(raster_tex));
    let texels_per_pixel = max(length(dx * size), length(dy * size)) * scale;
    // One texel per pixel, with margin for the rounding of derivatives, down to the ramp's end.
    let weight = clamp((1.25 - texels_per_pixel) / 0.2, 0.0, 1.0);
    return uv + weight * (dx * to_centre.x + dy * to_centre.y);
}

// The raster at `uv`, over a footprint of `dx` and `dy`, as linear colour with straight alpha. A
// raster with mip levels is filtered to the footprint.
fn raster_texel(uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    let texel = textureSampleGrad(raster_tex, raster_sampler, uv, dx, dy);
    return vec4<f32>(max(texel.rgb, vec3<f32>(0.0)) / max(texel.a, 1e-6), texel.a);
}

// The normal map's normal at `uv`, tilting `n` in the tangent frame the derivatives of the
// surface's position and texture coordinates give, or `n` itself where they give none.
fn mapped_normal(
    n: vec3<f32>,
    uv: vec2<f32>,
    world_dx: vec3<f32>,
    world_dy: vec3<f32>,
    uv_dx: vec2<f32>,
    uv_dy: vec2<f32>,
    scale: f32,
) -> vec3<f32> {
    let texel = textureSampleGrad(normal_map, normal_sampler, uv, uv_dx, uv_dy).xyz * 2.0 - 1.0;
    let dy_perp = cross(world_dy, n);
    let dx_perp = cross(n, world_dx);
    // Solving for the frame divides by this determinant. Only its sign is kept, which orients the
    // frame; with `dpdy` pointing down the screen it is negative where `n` faces the viewer.
    let side = select(1.0, -1.0, dot(n, cross(world_dx, world_dy)) < 0.0);
    let t = (dy_perp * uv_dx.x + dx_perp * uv_dy.x) * side;
    let b = (dy_perp * uv_dx.y + dx_perp * uv_dy.y) * side;
    let size = max(dot(t, t), dot(b, b));
    if (size <= 0.0) {
        return n;
    }
    let frame = inverseSqrt(size);
    // A normal map's +Y points up the image, towards lower V.
    let tilted = unit((t * texel.x - b * texel.y) * frame * scale + n * texel.z);
    return select(n, tilted, any(tilted != vec3<f32>(0.0)));
}

fn has(flags: u32, flag: u32) -> bool {
    return (flags & flag) != 0u;
}

// `p` seen from `view`: its place in texels of an atlas `size` texels across, and its depth.
fn shadow_coords(view: ShadowView, size: vec2<f32>, p: vec3<f32>) -> vec3<f32> {
    let clip = view.view_proj * vec4<f32>(p, 1.0);
    let ndc = clip.xy / clip.w;
    let uv = mix(view.tile.xy, view.tile.zw, vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5));
    return vec3<f32>(uv * size, dot(view.depth.xyz, p) + view.depth.w);
}

// How much of `light` reaches `world`, on a face whose geometric normal `face` points to the lit
// side, past whatever its shadow map holds, filtered over a disc of taps the size of the light's
// softness. The taps turn from pixel to pixel by `pixel`, so that supersampling smooths them.
fn shadow_visibility(light: Light, world: vec3<f32>, face: vec3<f32>, pixel: vec2<f32>) -> f32 {
    if (light.shadow < 0) {
        return 1.0;
    }
    // A directional light's shadows fade out over the last tenth of their distance.
    var fade = 1.0;
    if (light.kind == LIGHT_DIRECTIONAL) {
        let depth = dot(globals.camera_position - world, globals.camera_back);
        fade = clamp((light.shadow_distance - depth) / (0.1 * light.shadow_distance), 0.0, 1.0);
        if (fade <= 0.0) {
            return 1.0;
        }
    }
    var index = u32(light.shadow);
    if (light.kind == LIGHT_POINT) {
        // The face of the six the point lies most in the direction of.
        let d = world - light.position;
        let a = abs(d);
        if (a.x >= a.y && a.x >= a.z) {
            index += select(1u, 0u, d.x > 0.0);
        } else if (a.y >= a.z) {
            index += select(3u, 2u, d.y > 0.0);
        } else {
            index += select(5u, 4u, d.z > 0.0);
        }
    }
    let view = shadow_views[index];
    let texel = view.texel * select(1.0, max(dot(view.depth.xyz, world) + view.depth.w, 0.0), view.perspective != 0u);
    // Lifted off the surface by about a texel, so it doesn't shadow itself where a texel of the
    // map spans depths across a tilted surface.
    let p = world + face * texel * 1.5;
    let clip = view.view_proj * vec4<f32>(p, 1.0);
    if (clip.w <= 0.0 || any(abs(clip.xy) > vec2<f32>(clip.w))) {
        return 1.0;
    }
    let size = vec2<f32>(textureDimensions(shadow_map));
    let center = shadow_coords(view, size, p);
    if (center.z >= 1.0) {
        return 1.0;
    }
    // How the surface's depth changes across the map, so each tap compares against the depth of
    // the surface's own plane there, and a wide filter doesn't shadow the surface with itself.
    let axis = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(face.y) > 0.9);
    let t1 = normalize(cross(face, axis));
    let t2 = cross(face, t1);
    let a = shadow_coords(view, size, p + t1 * texel) - center;
    let b = shadow_coords(view, size, p + t2 * texel) - center;
    let det = a.x * b.y - b.x * a.y;
    let texel_depth = texel * length(view.depth.xyz);
    let radius = clamp(light.shadow_softness / texel, 1.0, MAX_SHADOW_RADIUS);
    // Each tap blends the four texels around it, up to a texel away along each axis.
    var bias = 0.1 * texel_depth;
    var gradient = vec2<f32>(0.0);
    if (abs(det) > 1e-8) {
        gradient = vec2<f32>(a.z * b.y - b.z * a.y, b.z * a.x - a.z * b.x) / det;
        // Followed only up to about 87° from the light, where a curved surface's plane strays
        // far from it within the filter, and the rest is taken up as bias instead.
        let limit = 20.0 * texel_depth;
        let steepness = length(gradient);
        if (steepness > limit) {
            gradient *= limit / steepness;
            bias += (steepness - limit) * (radius + 1.0);
        }
        bias += abs(gradient.x) + abs(gradient.y);
    }
    let low = view.tile.xy * size + 0.5;
    let high = view.tile.zw * size - 0.5;
    // Interleaved gradient noise.
    let turn = 2.0 * PI * fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
    var lit = 0.0;
    for (var i = 0u; i < SHADOW_TAPS; i += 1u) {
        let r = sqrt((f32(i) + 0.5) / f32(SHADOW_TAPS)) * radius;
        let angle = f32(i) * 2.39996323 + turn;
        let at = clamp(center.xy + r * vec2<f32>(cos(angle), sin(angle)), low, high);
        // Past the far end, where nothing was drawn, the surface's plane is lit.
        let reference = min(center.z + dot(gradient, at - center.xy) - bias, 1.0);
        lit += textureSampleCompareLevel(shadow_map, shadow_sampler, at / size, reference);
    }
    return mix(1.0, lit / f32(SHADOW_TAPS), fade);
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Derivatives need uniform control flow, so they come before anything branches.
    let uv_dx = dpdx(in.uv);
    let uv_dy = dpdy(in.uv);
    let uvs = Uvs(in.uv, uv_dx, uv_dy, in.uv1, dpdx(in.uv1), dpdy(in.uv1));
    let world_dx = dpdx(in.world);
    let world_dy = dpdy(in.world);
    let o = objects[in.object];
    let raster = has(o.flags, FLAG_RASTER);
    var texel = vec4<f32>(1.0);
    if (raster) {
        texel = raster_texel(pixel_centre_uv(in.clip.xy, in.uv, uv_dx, uv_dy), uv_dx, uv_dy);
    }
    var emissive = o.emissive * texel.rgb;
    if (has(o.flags, FLAG_EMISSIVE_MAP)) {
        let at = map_coords(o, SLOT_EMISSIVE, uvs);
        emissive *= textureSampleGrad(emissive_map, emissive_sampler, at.uv, at.dx, at.dy).rgb;
    }
    if (has(o.flags, FLAG_BASE_COLOR_MAP)) {
        let at = map_coords(o, SLOT_BASE_COLOR, uvs);
        texel *= textureSampleGrad(base_color_map, base_color_sampler, at.uv, at.dx, at.dy);
    }
    var base = o.base_color * texel * in.color;
    // A shadow catcher shows none of its colour, and catches shadows as much as its alpha covers
    // it, with its raster's and maps', which hide nothing where they are 0.
    let catcher = has(o.flags, FLAG_SHADOW_CATCHER);
    let coverage = base.a;
    if (catcher && coverage <= 0.0) {
        discard;
    }
    if (has(o.flags, FLAG_MASK)) {
        if (base.a < o.alpha_cutoff) {
            discard;
        }
        base.a = 1.0;
    } else if (!has(o.flags, FLAG_BLEND)) {
        base.a = 1.0;
    }
    if (has(o.flags, FLAG_UNLIT) && !catcher) {
        // A raster shown unlit keeps its colours as in 2D, whatever the exposure and tone mapping.
        let shown = base.rgb + emissive;
        return vec4<f32>(fogged(select(display(shown), shown, raster), in.world) * base.a, base.a);
    }

    let v = view_direction(in.world);
    let n = shading_normal(o, in.normal, v, front, uvs, world_dx, world_dy);
    // The face's own normal on the side `n` is lit from, which shadows are tested off.
    var face = unit(cross(world_dx, world_dy));
    face = select(n, select(face, -face, dot(face, n) < 0.0), any(face != vec3<f32>(0.0)));
    let receives = has(o.flags, FLAG_RECEIVE_SHADOWS);
    let n_dot_v = max(dot(n, v), 1e-4);
    var metallic = o.metallic;
    var roughness = o.roughness;
    if (has(o.flags, FLAG_METALLIC_ROUGHNESS_MAP)) {
        let at = map_coords(o, SLOT_METALLIC_ROUGHNESS, uvs);
        let texel = textureSampleGrad(metallic_roughness_map, metallic_roughness_sampler, at.uv, at.dx, at.dy);
        roughness *= texel.g;
        metallic *= texel.b;
    }
    metallic = clamp(metallic, 0.0, 1.0);
    roughness = clamp(roughness, 0.03, 1.0);
    // How much of the light from all around reaches the surface.
    var occlusion = 1.0;
    if (has(o.flags, FLAG_OCCLUSION_MAP)) {
        let at = map_coords(o, SLOT_OCCLUSION, uvs);
        let texel = textureSampleGrad(occlusion_map, occlusion_sampler, at.uv, at.dx, at.dy);
        occlusion = 1.0 + o.occlusion_strength * (texel.r - 1.0);
    }
    let alpha = roughness * roughness;
    let diffuse_color = base.rgb * (1.0 - metallic);
    let f0 = mix(vec3<f32>(0.04), base.rgb, metallic);

    var color = vec3<f32>(0.0);
    // Light reaching the surface with and without shadows, for a shadow catcher.
    var unshadowed = 0.0;
    var shadowed = 0.0;
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
        var visibility = 1.0;
        if (receives) {
            visibility = shadow_visibility(light, in.world, face, in.clip.xy);
        }
        let strength = luminance(radiance) * n_dot_l;
        unshadowed += strength;
        shadowed += strength * visibility;
        radiance *= visibility;
        let h = normalize(l + v);
        let n_dot_h = max(dot(n, h), 0.0);
        let v_dot_h = max(dot(v, h), 0.0);
        let fresnel = fresnel_schlick(f0, v_dot_h);
        let specular = fresnel * distribution_ggx(n_dot_h, alpha) * visibility_smith(n_dot_v, n_dot_l, alpha);
        let diffuse = (vec3<f32>(1.0) - fresnel) * diffuse_color / PI;
        color += (diffuse + specular) * radiance * n_dot_l;
    }

    if (catcher) {
        // Light from all around, as irradiance like the lights'.
        let around = luminance(diffuse_environment(n)) * PI;
        let shadow = select(0.0, 1.0 - (around + shadowed) / (around + unshadowed), unshadowed > 0.0);
        return vec4<f32>(0.0, 0.0, 0.0, clamp(shadow * coverage * fog_transmittance(in.world), 0.0, 1.0));
    }
    let reflected = reflect(-v, n);
    let blurred = normalize(mix(reflected, n, alpha * alpha));
    if (environment.map != 0u) {
        // What the surface reflects of light from all around, and what passes into its diffuse
        // layer.
        let split = environment_split_sum(n_dot_v, roughness);
        let reflectance = f0 * split.x + split.y;
        color += diffuse_color * diffuse_environment(n) * (vec3<f32>(1.0) - reflectance) * occlusion;
        color += specular_environment(blurred, roughness) * reflectance
            * specular_occlusion(n_dot_v, occlusion, roughness);
    } else {
        color += diffuse_color * diffuse_environment(n) * occlusion;
        color += specular_environment(blurred, roughness) * env_brdf(f0, roughness, n_dot_v) * occlusion;
    }
    color += emissive;
    return vec4<f32>(fogged(display(color), in.world) * base.a, base.a);
}

// The normal `o` is lit by where its interpolated vertex normal is `normal` and the viewer is
// along `v`, on its front or back as `front` says.
fn shading_normal(
    o: Object,
    normal: vec3<f32>,
    v: vec3<f32>,
    front: bool,
    uvs: Uvs,
    world_dx: vec3<f32>,
    world_dy: vec3<f32>,
) -> vec3<f32> {
    // Whether this is the back of the surface by its winding, which is lit by its normal reversed.
    let back = front == has(o.flags, FLAG_MIRRORED);
    // A normal the world matrix flattened away, on a face it left without area, faces the viewer.
    var n = v;
    if (has(o.flags, FLAG_FLAT_NORMALS)) {
        let face = unit(cross(world_dx, world_dy));
        if (any(face != vec3<f32>(0.0))) {
            n = select(face, -face, dot(face, v) < 0.0);
        }
    } else {
        let vertex = unit(normal);
        if (any(vertex != vec3<f32>(0.0))) {
            n = select(vertex, -vertex, back);
        }
    }
    if (has(o.flags, FLAG_NORMAL_MAP)) {
        // The map tilts the surface's own side; its back is lit by that tilted normal reversed.
        let side = select(1.0, -1.0, back);
        let at = map_coords(o, SLOT_NORMAL, uvs);
        n = side * mapped_normal(side * n, at.uv, world_dx, world_dy, at.dx, at.dy, o.normal_scale);
    }
    return n;
}

// Depth output where the surface is solid: the distance along the camera's view, from 0 at
// `depth_near` to 1 at `depth_far`, as opaque grey.
@fragment
fn fs_depth(in: VsOut) -> @location(0) vec4<f32> {
    if (!solid(objects[in.object], in.uv, in.uv1, in.color.a)) {
        discard;
    }
    let distance = dot(globals.camera_position - in.world, globals.camera_back);
    let depth = clamp((distance - globals.depth_near) / (globals.depth_far - globals.depth_near), 0.0, 1.0);
    return vec4<f32>(vec3<f32>(depth), 1.0);
}

// Normal output where the surface is solid: the normal it is lit by, in the camera's space, mapped
// from -1..1 to 0..1, and opaque.
@fragment
fn fs_normal(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Derivatives need uniform control flow, so they come before anything branches.
    let uvs = Uvs(in.uv, dpdx(in.uv), dpdy(in.uv), in.uv1, dpdx(in.uv1), dpdy(in.uv1));
    let world_dx = dpdx(in.world);
    let world_dy = dpdy(in.world);
    let o = objects[in.object];
    if (!solid(o, in.uv, in.uv1, in.color.a)) {
        discard;
    }
    let n = shading_normal(o, in.normal, view_direction(in.world), front, uvs, world_dx, world_dy);
    let camera = vec3<f32>(dot(n, globals.camera_right), dot(n, globals.camera_up), dot(n, globals.camera_back));
    return vec4<f32>(camera * 0.5 + 0.5, 1.0);
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// How much of a surface at `world` shows through the fog.
fn fog_transmittance(world: vec3<f32>) -> f32 {
    let to_camera = globals.camera_position - world;
    let distance = select(length(to_camera), dot(to_camera, globals.camera_back), globals.orthographic != 0u);
    return exp(-globals.fog_density * max(distance, 0.0));
}

// `color`, already in the displayed range, faded into the fog by the distance to `world`.
fn fogged(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    if (globals.fog_density <= 0.0) {
        return color;
    }
    return mix(globals.fog_color, color, fog_transmittance(world));
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
