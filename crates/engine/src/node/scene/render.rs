//! Records `render_3d`: the scene's shadow maps, depth-tested draws of its objects into a
//! supersampled float target, the environment map behind them if it shows, then a resolve into
//! the node's raster. Depth and normal output draw the objects once at the output's size, unlit,
//! then store what they drew.

use bytemuck::{Pod, Zeroable};
use lumen_3d::{
    Blending, Bounds, Camera, DrawKey, Draws, Filter, LightKind, Lighting, Map, MapUv, Object,
    Projection, Row, Sampler, Scene, ShadowAtlas, Wrap,
};
use lumen_gpu::{
    DEPTH_FORMAT, DepthTest, LayoutEntry, LayoutKind, RenderTargetSpec, Res, Size, TargetLoad,
    TexRef, TextureDomain, wgpu,
};

use super::collect::{Collected, GpuMesh, solid_linear};
use super::{Light3d, Render3d, Render3dOutput, Render3dOutputDelegate};
use crate::error::{LumenError, RenderError};
use crate::gpu::frame_graph::{FrameCx, Programs, Raster, RecordNode};
use crate::node::{Deferred, DelegateEvalContext, DelegateEvaluable, required_input};

pub(crate) const SHADER: &str = include_str!("scene.wgsl");
const RESOLVE_SHADER: &str = include_str!("resolve.wgsl");
const DATA_SHADER: &str = include_str!("data.wgsl");
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

const VERTEX_FRAGMENT: wgpu::ShaderStages =
    wgpu::ShaderStages::VERTEX.union(wgpu::ShaderStages::FRAGMENT);
const LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::staged(0, VERTEX_FRAGMENT, LayoutKind::Uniform),
    LayoutEntry::staged(1, VERTEX_FRAGMENT, LayoutKind::Storage),
    LayoutEntry::staged(2, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(3, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(4, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
    LayoutEntry::staged(5, wgpu::ShaderStages::VERTEX, LayoutKind::Uniform),
    LayoutEntry::staged(6, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(7, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(8, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(9, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(10, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(11, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(12, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(13, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(14, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(15, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(16, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(17, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(18, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(19, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(20, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
    LayoutEntry::staged(21, wgpu::ShaderStages::FRAGMENT, LayoutKind::DepthTexture),
    LayoutEntry::staged(
        22,
        wgpu::ShaderStages::FRAGMENT,
        LayoutKind::ComparisonSampler,
    ),
    LayoutEntry::staged(23, wgpu::ShaderStages::FRAGMENT, LayoutKind::Uniform),
    LayoutEntry::staged(24, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(26, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(27, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
    LayoutEntry::staged(28, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(29, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
];
/// What drawing depth or normals binds: the vertices, what cuts out masked and blended surfaces,
/// and the normal map.
const DATA_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::staged(0, VERTEX_FRAGMENT, LayoutKind::Uniform),
    LayoutEntry::staged(1, VERTEX_FRAGMENT, LayoutKind::Storage),
    LayoutEntry::staged(2, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(3, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(5, wgpu::ShaderStages::VERTEX, LayoutKind::Uniform),
    LayoutEntry::staged(6, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(7, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(8, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(9, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(10, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(12, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(15, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(17, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(28, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(29, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
];
/// What drawing the environment behind the scene binds.
const BACKGROUND_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::staged(0, wgpu::ShaderStages::FRAGMENT, LayoutKind::Uniform),
    LayoutEntry::staged(23, wgpu::ShaderStages::FRAGMENT, LayoutKind::Uniform),
    LayoutEntry::staged(24, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(25, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(26, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
];
/// What drawing into the shadow atlas binds: the vertices, what cuts out masked and blended
/// surfaces, and the views.
const SHADOW_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::staged(1, VERTEX_FRAGMENT, LayoutKind::Storage),
    LayoutEntry::staged(2, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(3, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(5, wgpu::ShaderStages::VERTEX, LayoutKind::Uniform),
    LayoutEntry::staged(6, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(7, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(8, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(9, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(10, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(15, wgpu::ShaderStages::FRAGMENT, LayoutKind::Sampler),
    LayoutEntry::staged(20, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(28, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(29, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
];

const CLAMP_SAMPLER: u64 = 0x3d00_0001;
const SHADOW_SAMPLER: u64 = 0x3d00_0002;
const ENVIRONMENT_SAMPLER: u64 = 0x3d00_0003;
/// Map samplers' keys are this with their settings in the low byte.
const MAP_SAMPLERS: u64 = 0x3d01_0000;

pub(crate) const FLAG_UNLIT: u32 = 1;
pub(crate) const FLAG_BLEND: u32 = 2;
pub(crate) const FLAG_MIRRORED: u32 = 4;
/// The bound texture is the object's raster.
pub(crate) const FLAG_RASTER: u32 = 8;
pub(crate) const FLAG_MASK: u32 = 16;
pub(crate) const FLAG_SKINNED: u32 = 32;
pub(crate) const FLAG_FLAT_NORMALS: u32 = 64;
pub(crate) const FLAG_BASE_COLOR_MAP: u32 = 128;
pub(crate) const FLAG_METALLIC_ROUGHNESS_MAP: u32 = 256;
pub(crate) const FLAG_NORMAL_MAP: u32 = 512;
pub(crate) const FLAG_EMISSIVE_MAP: u32 = 1024;
pub(crate) const FLAG_OCCLUSION_MAP: u32 = 2048;
pub(crate) const FLAG_RECEIVE_SHADOWS: u32 = 4096;
pub(crate) const FLAG_SHADOW_CATCHER: u32 = 8192;
pub(crate) const FLAG_VERTEX_COLORS: u32 = 16384;
/// The object's maps read through its rows of the map coordinates table.
pub(crate) const FLAG_MAP_UVS: u32 = 32768;
pub(crate) const LIGHT_DIRECTIONAL: u32 = 0;
pub(crate) const LIGHT_POINT: u32 = 1;
pub(crate) const LIGHT_SPOT: u32 = 2;

/// Keep in sync with `Globals` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuGlobals {
    pub(crate) view_proj: [[f32; 4]; 4],
    pub(crate) camera_position: [f32; 3],
    pub(crate) light_count: u32,
    pub(crate) camera_back: [f32; 3],
    pub(crate) orthographic: u32,
    pub(crate) ambient: [f32; 3],
    pub(crate) environment: f32,
    pub(crate) fog_color: [f32; 3],
    pub(crate) fog_density: f32,
    pub(crate) tone_mapping: u32,
    pub(crate) exposure: f32,
    pub(crate) supersample: u32,
    _pad: u32,
    pub(crate) camera_right: [f32; 3],
    pub(crate) depth_near: f32,
    pub(crate) camera_up: [f32; 3],
    pub(crate) depth_far: f32,
}

/// Keep in sync with `Environment` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuEnvironment {
    pub(crate) irradiance: [[f32; 4]; 9],
    pub(crate) clip_to_world: [[f32; 4]; 4],
    pub(crate) turn: [f32; 2],
    pub(crate) roughest: f32,
    pub(crate) background_blur: f32,
    pub(crate) map: u32,
    _pad: [u32; 3],
}

/// Keep in sync with `Light` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuLight {
    pub(crate) position: [f32; 3],
    pub(crate) kind: u32,
    pub(crate) direction: [f32; 3],
    pub(crate) range: f32,
    pub(crate) radiance: [f32; 3],
    pub(crate) cos_outer: f32,
    pub(crate) cos_inner: f32,
    /// The light's first view in the shadow views, or -1 for a light without shadows.
    pub(crate) shadow: i32,
    pub(crate) shadow_softness: f32,
    pub(crate) shadow_distance: f32,
}

/// Keep in sync with `ShadowView` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuShadowView {
    pub(crate) view_proj: [[f32; 4]; 4],
    pub(crate) depth: [f32; 4],
    /// The view's tile as fractions of the atlas: left, top, right and bottom.
    pub(crate) tile: [f32; 4],
    pub(crate) texel: f32,
    pub(crate) perspective: u32,
    _pad: [u32; 2],
}

/// One row of the object table; keep in sync with `Object` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuObject {
    pub(crate) model: [[f32; 4]; 4],
    /// The columns of the normal matrix, each padded to four floats.
    pub(crate) normal: [[f32; 4]; 3],
    pub(crate) base_color: [f32; 4],
    pub(crate) emissive: [f32; 3],
    pub(crate) metallic: f32,
    pub(crate) roughness: f32,
    pub(crate) flags: u32,
    pub(crate) first_vertex: u32,
    pub(crate) first_index: u32,
    pub(crate) first_joint: u32,
    pub(crate) alpha_cutoff: f32,
    pub(crate) normal_scale: f32,
    pub(crate) occlusion_strength: f32,
    /// Where the mesh's vertex extras start in its vertex data, in words, or `u32::MAX` for a
    /// mesh without any.
    pub(crate) extras: u32,
    /// Where the object's morph target displacements start in its mesh's vertex data, in words.
    pub(crate) first_delta: u32,
    /// The mesh vertex whose displacements come first.
    pub(crate) morph_vertex: u32,
    /// 0 for an object without morph targets.
    pub(crate) morph_targets: u32,
    pub(crate) first_weight: u32,
    /// Where the object's five rows of the map coordinates table start.
    pub(crate) first_map_uv: u32,
    _pad: [u32; 2],
}

/// One map's row of the map coordinates table; keep in sync with `MapUv` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuMapUv {
    /// The transform's first row, then the set of texture coordinates it reads.
    pub(crate) first: [f32; 4],
    pub(crate) second: [f32; 4],
}

/// Keep in sync with `Run` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuRun {
    pub(crate) first: u32,
    pub(crate) count: u32,
    pub(crate) base: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ResolveParams {
    supersample: u32,
    _pad: [u32; 3],
}

/// Keep in sync with `Params` in `data.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DataParams {
    background: [f32; 4],
    drawn: u32,
    _pad: [u32; 3],
}

fn spec(depth: DepthTest, cull: Option<wgpu::Face>) -> RenderTargetSpec {
    RenderTargetSpec {
        format: Some(COLOR_FORMAT),
        blend: wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        depth: Some(depth),
        cull,
    }
}

/// The background goes under what the scene drew, showing through as much as it leaves
/// uncovered.
fn background_spec() -> RenderTargetSpec {
    let under = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    RenderTargetSpec::flat(
        COLOR_FORMAT,
        wgpu::BlendState {
            color: under,
            alpha: under,
        },
    )
}

/// The label and fragment entry point of the programs drawing depth or normal output, and what
/// the raster holds where nothing is drawn.
type DataProgram = (&'static str, &'static str, [f32; 4]);

/// How `output` is drawn, unless it is colour.
fn data_program(output: Render3dOutput) -> Option<DataProgram> {
    match output {
        Render3dOutput::Color => None,
        Render3dOutput::Depth => Some(("scene-3d-depth", "fs_depth", [1.0; 4])),
        Render3dOutput::Normal => Some(("scene-3d-normal", "fs_normal", [0.0; 4])),
    }
}

/// Depth and normals replace what is behind them, whatever the object's blending.
fn data_spec(cull: Option<wgpu::Face>) -> RenderTargetSpec {
    RenderTargetSpec {
        blend: wgpu::BlendState::REPLACE,
        ..spec(DepthTest::Write, cull)
    }
}

/// The faces a run culls: none for a double-sided one, else those pointing away, which wind
/// clockwise, or counterclockwise once mirrored.
fn cull(key: &DrawKey<GpuMesh, TexRef>) -> Option<wgpu::Face> {
    match (key.double_sided, key.mirrored) {
        (true, _) => None,
        (false, false) => Some(wgpu::Face::Back),
        (false, true) => Some(wgpu::Face::Front),
    }
}

/// Shadow catchers replace what is under them, so the nearest of overlapping ones shows alone.
fn catcher_spec(cull: Option<wgpu::Face>) -> RenderTargetSpec {
    RenderTargetSpec {
        blend: wgpu::BlendState::REPLACE,
        ..spec(DepthTest::Write, cull)
    }
}

/// Programs differing only by blending need a variant apart.
const CATCHER_VARIANT: u64 = 1;

/// Opaque runs test and write depth, blended ones only test it. Single-sided runs cull the faces
/// that point away, which wind clockwise, or counterclockwise once mirrored.
const VARIANTS: [(DepthTest, Option<wgpu::Face>); 6] = [
    (DepthTest::Write, Some(wgpu::Face::Back)),
    (DepthTest::Write, Some(wgpu::Face::Front)),
    (DepthTest::Write, None),
    (DepthTest::Read, Some(wgpu::Face::Back)),
    (DepthTest::Read, Some(wgpu::Face::Front)),
    (DepthTest::Read, None),
];

/// `object`'s row of the object table, adding its maps' rows of the map coordinates table to
/// `map_uvs` when any moves its coordinates or reads the second set.
fn gpu_object(
    object: &Object<GpuMesh, TexRef>,
    row: &Row,
    map_uvs: &mut Vec<GpuMapUv>,
) -> GpuObject {
    let surface = &object.surface;
    let mut flags = 0;
    let first_map_uv = map_uvs.len() as u32;
    if surface.map_uvs != [MapUv::default(); 5] {
        flags |= FLAG_MAP_UVS;
        map_uvs.extend(surface.map_uvs.iter().map(|uv| {
            let [first, second] = uv.transform;
            GpuMapUv {
                first: [first[0], first[1], first[2], f32::from(uv.set)],
                second: [second[0], second[1], second[2], 0.0],
            }
        }));
    }
    if surface.vertex_colors {
        flags |= FLAG_VERTEX_COLORS;
    }
    if surface.unlit {
        flags |= FLAG_UNLIT;
    }
    let mut alpha_cutoff = 0.0;
    match surface.blending {
        Blending::Opaque => {}
        Blending::Mask { cutoff } => {
            flags |= FLAG_MASK;
            alpha_cutoff = cutoff;
        }
        Blending::Blend => flags |= FLAG_BLEND,
    }
    if object.first_joint.is_some() {
        flags |= FLAG_SKINNED;
    }
    if surface.flat_normals {
        flags |= FLAG_FLAT_NORMALS;
    }
    let maps = &surface.maps;
    for (map, flag) in [
        (&maps.base_color, FLAG_BASE_COLOR_MAP),
        (&maps.metallic_roughness, FLAG_METALLIC_ROUGHNESS_MAP),
        (&maps.normal, FLAG_NORMAL_MAP),
        (&maps.emissive, FLAG_EMISSIVE_MAP),
        (&maps.occlusion, FLAG_OCCLUSION_MAP),
    ] {
        if map.is_some() {
            flags |= flag;
        }
    }
    if lumen_3d::mirrors(&object.world) {
        flags |= FLAG_MIRRORED;
    }
    if surface.receive_shadows {
        flags |= FLAG_RECEIVE_SHADOWS;
    }
    if surface.shadow_catcher {
        flags |= FLAG_SHADOW_CATCHER | FLAG_RECEIVE_SHADOWS;
    }
    if surface.texture.is_some() {
        flags |= FLAG_RASTER;
    }
    let normal = row.normal;
    GpuObject {
        model: object.world.to_cols_array_2d(),
        normal: [normal.x_axis, normal.y_axis, normal.z_axis].map(|axis| axis.extend(0.0).into()),
        base_color: surface.base_color,
        emissive: surface.emissive,
        metallic: surface.metallic,
        roughness: surface.roughness,
        flags,
        first_vertex: row.first_vertex,
        first_index: object.first_index,
        first_joint: object.first_joint.unwrap_or(0),
        alpha_cutoff,
        normal_scale: surface.normal_scale,
        occlusion_strength: surface.occlusion_strength,
        extras: object.mesh.extras.unwrap_or(u32::MAX),
        first_delta: object.morph.map_or(0, |(morph, _)| {
            object.mesh.morphs + morph.first_delta * MORPH_DELTA_WORDS
        }),
        morph_vertex: object.morph.map_or(0, |(morph, _)| morph.first_vertex),
        morph_targets: object.morph.map_or(0, |(morph, _)| morph.targets),
        first_weight: object.morph.map_or(0, |(_, weight)| weight),
        first_map_uv,
        _pad: [0; 2],
    }
}

/// The words of one [`lumen_3d::MorphDelta`].
pub(crate) const MORPH_DELTA_WORDS: u32 = (size_of::<lumen_3d::MorphDelta>() / 4) as u32;

fn sampler_key(sampler: &Sampler) -> u64 {
    let wrap = |wrap: Wrap| match wrap {
        Wrap::Repeat => 0,
        Wrap::MirroredRepeat => 1,
        Wrap::ClampToEdge => 2,
    };
    let filter = |filter: Filter| match filter {
        Filter::Nearest => 0,
        Filter::Linear => 1,
    };
    let mipmap = sampler.mipmap.map_or(0, |mipmap| 1 + filter(mipmap));
    MAP_SAMPLERS
        | wrap(sampler.wrap_u)
        | wrap(sampler.wrap_v) << 2
        | filter(sampler.mag) << 4
        | filter(sampler.min) << 5
        | mipmap << 6
}

fn sampler_descriptor(sampler: &Sampler) -> wgpu::SamplerDescriptor<'static> {
    let wrap = |wrap: Wrap| match wrap {
        Wrap::Repeat => wgpu::AddressMode::Repeat,
        Wrap::MirroredRepeat => wgpu::AddressMode::MirrorRepeat,
        Wrap::ClampToEdge => wgpu::AddressMode::ClampToEdge,
    };
    let filter = |filter: Filter| match filter {
        Filter::Nearest => wgpu::FilterMode::Nearest,
        Filter::Linear => wgpu::FilterMode::Linear,
    };
    wgpu::SamplerDescriptor {
        label: Some("lumen 3d map sampler"),
        address_mode_u: wrap(sampler.wrap_u),
        address_mode_v: wrap(sampler.wrap_v),
        mag_filter: filter(sampler.mag),
        min_filter: filter(sampler.min),
        mipmap_filter: match sampler.mipmap {
            Some(Filter::Linear) => wgpu::MipmapFilterMode::Linear,
            _ => wgpu::MipmapFilterMode::Nearest,
        },
        // Without mipmapping, only the full-size level is read.
        lod_max_clamp: if sampler.mipmap.is_some() { 32.0 } else { 0.0 },
        ..wgpu::SamplerDescriptor::default()
    }
}

/// `light`, whose shadows start at `shadow` among the shadow views, if it has any.
fn gpu_light(light: &lumen_3d::Light, shadow: Option<u32>) -> GpuLight {
    GpuLight {
        position: light.position.into(),
        kind: match light.kind {
            LightKind::Directional => LIGHT_DIRECTIONAL,
            LightKind::Point => LIGHT_POINT,
            LightKind::Spot => LIGHT_SPOT,
            LightKind::Ambient => unreachable!("ambient light is summed, not drawn"),
        },
        direction: light.direction.into(),
        range: light.range,
        radiance: light.radiance.into(),
        cos_outer: light.cos_outer,
        cos_inner: light.cos_inner,
        shadow: shadow.map_or(-1, |view| view as i32),
        shadow_softness: light.shadow.map_or(0.0, |shadow| shadow.softness),
        shadow_distance: light.shadow.map_or(0.0, |shadow| shadow.distance),
    }
}

fn gpu_shadow_views(atlas: &ShadowAtlas) -> Vec<GpuShadowView> {
    atlas
        .views
        .iter()
        .enumerate()
        .map(|(index, view)| GpuShadowView {
            view_proj: view.view_proj.to_cols_array_2d(),
            depth: view.depth.into(),
            tile: atlas.tile_rect(index),
            texel: view.texel,
            perspective: u32::from(view.perspective),
            _pad: [0; 2],
        })
        .collect()
}

/// The shadow views of `lights` over the objects `draws` draws, when any of them casts shadows.
fn shadow_atlas(
    objects: &[Object<GpuMesh, TexRef>],
    draws: &Draws<GpuMesh, TexRef>,
    lights: &[lumen_3d::Light],
    camera: &Camera,
    aspect: f32,
    max_side: u32,
) -> Option<ShadowAtlas> {
    if !draws.runs.iter().any(|run| run.key.cast_shadows) {
        return None;
    }
    let (mut casters, mut receivers) = (Bounds::EMPTY, Bounds::EMPTY);
    for object in draws.rows.iter().map(|row| &objects[row.object]) {
        let surface = &object.surface;
        if surface.cast_shadows && !surface.shadow_catcher {
            casters = casters.union(object.bounds);
        }
        if (surface.receive_shadows && !surface.unlit) || surface.shadow_catcher {
            receivers = receivers.union(object.bounds);
        }
    }
    ShadowAtlas::plan(lights, camera, aspect, casters, receivers, max_side)
}

pub(super) fn render_error(node_id: crate::node::NodeId, details: String) -> LumenError {
    RenderError::NodeEvaluation {
        node_id,
        node_kind: "Render3d",
        details,
    }
    .into()
}

/// Whether `flag` may be on at some frame.
fn may(flag: &Deferred<bool>) -> bool {
    !matches!(flag, Deferred::Value(false))
}

/// Builds the program drawing shadow maps, when `light` may cast shadows.
pub(crate) fn light_programs(light: &Light3d, programs: &mut Programs<'_>) -> crate::Result<()> {
    if may(&light.params.cast_shadows) {
        programs.render(
            "scene-3d-shadow",
            SHADER,
            "vs_shadow",
            "fs_shadow",
            SHADOW_LAYOUT,
            RenderTargetSpec::depth_only(DepthTest::Write, None),
            0,
        )?;
    }
    Ok(())
}

/// Builds the programs drawing a surface as a shadow catcher, facing either way or both, when
/// `shadow_catcher` may be on.
pub(crate) fn catcher_programs(
    shadow_catcher: &Deferred<bool>,
    programs: &mut Programs<'_>,
) -> crate::Result<()> {
    if may(shadow_catcher) {
        for cull in [Some(wgpu::Face::Back), Some(wgpu::Face::Front), None] {
            programs.render(
                "scene-3d",
                SHADER,
                "vs_main",
                "fs_main",
                LAYOUT,
                catcher_spec(cull),
                CATCHER_VARIANT,
            )?;
        }
    }
    Ok(())
}

impl Render3d {
    /// Draws the depth or normals of `collected`'s objects at `size` with `program`, from
    /// [`data_program`].
    fn record_data(
        &self,
        cx: &mut FrameCx<'_>,
        params: &super::Render3dParams,
        collected: &Collected,
        size: Size,
        (label, fragment, background): DataProgram,
    ) -> crate::Result<Raster> {
        let scene = &collected.scene;
        let camera = scene.camera.unwrap_or_default();
        let (near, far) = depth_range(&camera, params);
        if params.output == Render3dOutput::Depth
            && far.partial_cmp(&near) != Some(std::cmp::Ordering::Greater)
        {
            return Err(render_error(
                self.id,
                format!("depth_far ({far}) must be farther than depth_near ({near})"),
            ));
        }
        let draws = Draws::new(&scene.objects, &camera.view())
            .map_err(|error| render_error(self.id, error.to_string()))?;
        let output = cx.storage_target(size);
        let unread = cx.transparent(size).tex;
        let data = DataParams {
            background,
            drawn: u32::from(!draws.runs.is_empty()),
            _pad: [0; 3],
        };
        if draws.runs.is_empty() {
            if params.output == Render3dOutput::Normal {
                return Ok(cx.transparent(size));
            }
            cx.spatial_pass("scene-3d-data", DATA_SHADER, unread, &data, output, size)?;
            return Ok(Raster {
                tex: output,
                domain: TextureDomain::full_frame(size),
                bounds: None,
            });
        }
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let no_lights = Lighting {
            direct: Vec::new(),
            ambient: lumen_3d::glam::Vec3::ZERO,
        };
        let globals = cx.rec.uniform(&globals(
            &camera, aspect, 0, &no_lights, params, [0.0; 4], 1,
        ));
        let tables = Tables::new(cx, scene, &draws);
        let target = cx.rec.acquire(
            size,
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = cx
            .rec
            .acquire(size, DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let clamp = clamp_sampler(cx);
        for (drawn, (run, &rows)) in draws.runs.iter().zip(&tables.runs).enumerate() {
            let program = cx.render_program(
                label,
                SHADER,
                "vs_main",
                fragment,
                DATA_LAYOUT,
                data_spec(cull(&run.key)),
                0,
            )?;
            let mut bindings = vec![
                (0, globals),
                (1, tables.objects),
                (2, Res::Buffer(run.key.mesh.vertices)),
                (3, Res::Buffer(run.key.mesh.indices)),
                (5, rows),
                (6, Res::Tex(run.key.texture.unwrap_or(unread))),
                (7, Res::Sampler(clamp)),
                (8, tables.joints),
                (
                    9,
                    Res::Buffer(run.key.mesh.data.unwrap_or(run.key.mesh.vertices)),
                ),
                (28, tables.weights),
                (29, tables.map_uvs),
            ];
            for (texture, sampler, map) in [
                (10, 15, run.key.maps.base_color),
                (12, 17, run.key.maps.normal),
            ] {
                let (tex, map_sampler) = match map {
                    Some(Map { texture, sampler }) => (
                        texture,
                        cx.rec
                            .sampler(sampler_key(&sampler), || sampler_descriptor(&sampler)),
                    ),
                    None => (unread, clamp),
                };
                bindings.push((texture, Res::Tex(tex)));
                bindings.push((sampler, Res::Sampler(map_sampler)));
            }
            let load = if drawn == 0 {
                TargetLoad::Clear
            } else {
                TargetLoad::Load
            };
            cx.rec.draw_with_depth(
                program,
                &bindings,
                target,
                load,
                depth,
                load,
                run.vertices,
                1,
            );
        }
        cx.spatial_pass("scene-3d-data", DATA_SHADER, target, &data, output, size)?;
        Ok(Raster {
            tex: output,
            domain: TextureDomain::full_frame(size),
            bounds: None,
        })
    }

    /// Whether some frame may draw the environment map behind the scene.
    fn may_show_background(&self) -> bool {
        let params = &self.params;
        !matches!(params.environment_background, Deferred::Value(false))
            && !matches!(&params.environment_map, Deferred::Value(map) if map.is_empty())
    }
}

impl RecordNode for Render3d {
    /// Builds the programs scenes draw with, and the background's if the scene may show one, but
    /// for those drawing shadow maps and catchers, which the lights and surfaces asking for them
    /// build.
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        let output = match self.params.output {
            Render3dOutputDelegate::Depth => Render3dOutput::Depth,
            Render3dOutputDelegate::Normal => Render3dOutput::Normal,
            _ => Render3dOutput::Color,
        };
        if let Some((label, fragment, _)) = data_program(output) {
            for cull in [Some(wgpu::Face::Back), Some(wgpu::Face::Front), None] {
                programs.render(
                    label,
                    SHADER,
                    "vs_main",
                    fragment,
                    DATA_LAYOUT,
                    data_spec(cull),
                    0,
                )?;
            }
            return programs.spatial("scene-3d-data", DATA_SHADER);
        }
        for (depth, cull) in VARIANTS {
            programs.render(
                "scene-3d",
                SHADER,
                "vs_main",
                "fs_main",
                LAYOUT,
                spec(depth, cull),
                0,
            )?;
        }
        if self.may_show_background() {
            programs.render(
                "scene-3d-background",
                SHADER,
                "vs_background",
                "fs_background",
                BACKGROUND_LAYOUT,
                background_spec(),
                0,
            )?;
        }
        programs.spatial("scene-3d-resolve", RESOLVE_SHADER)
    }

    fn record(&self, cx: &mut FrameCx<'_>) -> crate::Result<Raster> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let canvas = cx.canvas_size();
        let pick = |value: u32, fallback: u32| if value == 0 { fallback } else { value };
        let size = Size::new(
            pick(params.width, canvas.width),
            pick(params.height, canvas.height),
        );
        let max = cx.rec.device().limits().max_texture_dimension_2d;
        if size.width > max || size.height > max {
            return Err(render_error(
                self.id,
                format!(
                    "a {}x{} output is larger than this device's {max}-pixel texture limit",
                    size.width, size.height
                ),
            ));
        }
        let mut collected = Collected::new(self.id);
        collected.add(cx, required_input(self.id, "scene", self.scene)?)?;
        if let Some(program) = data_program(params.output) {
            return self.record_data(cx, &params, &collected, size, program);
        }
        if collected
            .scene
            .lights
            .iter()
            .any(|light| light.shadow.is_some())
        {
            collected.place_bounds();
        }
        let background = params.environment_background && !params.environment_map.is_empty();
        let environment =
            super::environment::environment(cx, self.id, &params.environment_map, background)?;
        let camera = collected.scene.camera.unwrap_or_default();
        // The output fits, so a supersample of 1 always does.
        let mut supersample = params.supersample.clamp(1, 4);
        while supersample > 1
            && size
                .width
                .max(size.height)
                .checked_mul(supersample)
                .is_none_or(|side| side > max)
        {
            supersample -= 1;
        }
        let target_size = Size::new(size.width * supersample, size.height * supersample);
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let mipped = mip_rasters(cx, &mut collected, &camera, aspect, target_size)?;
        let scene = &collected.scene;
        let fog = solid_linear(self.id, "Render3d", &params.fog_color)?;
        let view = camera.view();
        let draws = Draws::new(&scene.objects, &view)
            .map_err(|error| render_error(self.id, error.to_string()))?;
        if draws.runs.is_empty() && !background {
            return Ok(cx.transparent(size));
        }

        // An environment map lights a scene without lights in place of the default ones.
        let lighting = if environment.is_some() && scene.lights.is_empty() {
            Lighting {
                direct: Vec::new(),
                ambient: lumen_3d::glam::Vec3::ZERO,
            }
        } else {
            Lighting::of(&scene.lights)
        };
        let atlas = shadow_atlas(
            &scene.objects,
            &draws,
            &lighting.direct,
            &camera,
            aspect,
            max,
        );
        if let Some(atlas) = &atlas
            && !atlas.dropped.is_empty()
        {
            // Lighting keeps direct lights in scene order.
            let direct_nodes: Vec<_> = scene
                .lights
                .iter()
                .zip(&collected.light_nodes)
                .filter(|(light, _)| light.kind != LightKind::Ambient)
                .map(|(_, &node)| node)
                .collect();
            let mut named = std::collections::HashSet::new();
            let dropped = atlas
                .dropped
                .iter()
                .map(|&index| direct_nodes[index])
                .filter(|&node| named.insert(node))
                .collect();
            cx.note_shadow_limit(self.id, dropped);
        }
        let mut lights: Vec<GpuLight> = lighting
            .direct
            .iter()
            .enumerate()
            .map(|(index, light)| {
                gpu_light(
                    light,
                    atlas.as_ref().and_then(|atlas| atlas.first_view[index]),
                )
            })
            .collect();
        let light_count = lights.len() as u32;
        // A storage binding can't be empty.
        if lights.is_empty() {
            lights.push(GpuLight::zeroed());
        }
        let globals = cx.rec.uniform(&globals(
            &camera,
            aspect,
            light_count,
            &lighting,
            &params,
            fog,
            supersample,
        ));
        let lights = cx.rec.storage_parts(&[bytemuck::cast_slice(&lights)]);
        let Tables {
            objects,
            map_uvs,
            weights,
            joints,
            runs,
        } = Tables::new(cx, scene, &draws);

        let color = cx.rec.acquire(
            target_size,
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = cx.rec.acquire(
            target_size,
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let clamp = clamp_sampler(cx);
        let mipped_sampler = cx
            .rec
            .sampler(super::mips::RASTER_SAMPLER, super::mips::raster_sampler);
        // Objects without a raster or a map still bind a texture and sampler in its place, which
        // their shader doesn't read, as meshes without skins bind their vertices.
        let unread = cx.transparent(size).tex;
        let shadow_views = match &atlas {
            Some(atlas) => cx
                .rec
                .storage_parts(&[bytemuck::cast_slice(&gpu_shadow_views(atlas))]),
            None => cx
                .rec
                .storage_parts(&[bytemuck::bytes_of(&GpuShadowView::zeroed())]),
        };
        let shadow_map = match &atlas {
            Some(atlas) => {
                let (width, height) = atlas.size();
                let map = cx.rec.acquire(
                    Size::new(width, height),
                    DEPTH_FORMAT,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                );
                let program = cx.render_program(
                    "scene-3d-shadow",
                    SHADER,
                    "vs_shadow",
                    "fs_shadow",
                    SHADOW_LAYOUT,
                    RenderTargetSpec::depth_only(DepthTest::Write, None),
                    0,
                )?;
                let views = atlas.views.len() as u32;
                let casting = draws.runs.iter().zip(&runs);
                for (cast, (run, &rows)) in
                    casting.filter(|(run, _)| run.key.cast_shadows).enumerate()
                {
                    let (base_color, base_color_sampler) = match run.key.maps.base_color {
                        Some(Map { texture, sampler }) => (
                            texture,
                            cx.rec
                                .sampler(sampler_key(&sampler), || sampler_descriptor(&sampler)),
                        ),
                        None => (unread, clamp),
                    };
                    let bindings = [
                        (1, objects),
                        (2, Res::Buffer(run.key.mesh.vertices)),
                        (3, Res::Buffer(run.key.mesh.indices)),
                        (5, rows),
                        (6, Res::Tex(run.key.texture.unwrap_or(unread))),
                        (7, Res::Sampler(clamp)),
                        (8, joints),
                        (
                            9,
                            Res::Buffer(run.key.mesh.data.unwrap_or(run.key.mesh.vertices)),
                        ),
                        (10, Res::Tex(base_color)),
                        (15, Res::Sampler(base_color_sampler)),
                        (20, shadow_views),
                        (28, weights),
                        (29, map_uvs),
                    ];
                    let load = if cast == 0 {
                        TargetLoad::Clear
                    } else {
                        TargetLoad::Load
                    };
                    cx.rec
                        .draw_depth(program, &bindings, map, load, run.vertices, views);
                }
                map
            }
            None => {
                let (rec, cache) = cx.scene_cache();
                cache.no_shadows(rec)
            }
        };
        let shadow_sampler = cx.rec.sampler(SHADOW_SAMPLER, || wgpu::SamplerDescriptor {
            label: Some("lumen 3d shadow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..wgpu::SamplerDescriptor::default()
        });

        let environment_sampler = cx
            .rec
            .sampler(ENVIRONMENT_SAMPLER, || wgpu::SamplerDescriptor {
                label: Some("lumen 3d environment sampler"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..wgpu::SamplerDescriptor::default()
            });
        let environment_uniform = cx.rec.uniform(&gpu_environment(
            environment.as_ref(),
            &camera,
            aspect,
            &params,
        ));
        // Without a map, the shader reads none of these.
        let (environment_map, environment_brdf) = match &environment {
            Some(loaded) => {
                let (rec, cache) = cx.scene_cache();
                (
                    loaded.reflections,
                    Res::Buffer(cache.environments.brdf(rec)),
                )
            }
            None => (unread, lights),
        };

        for (drawn, (run, &rows)) in draws.runs.iter().zip(&runs).enumerate() {
            let depth_test = if run.key.blend {
                DepthTest::Read
            } else {
                DepthTest::Write
            };
            let cull = cull(&run.key);
            let (target, variant) = if run.key.shadow_catcher {
                (catcher_spec(cull), CATCHER_VARIANT)
            } else {
                (spec(depth_test, cull), 0)
            };
            let program = cx.render_program(
                "scene-3d", SHADER, "vs_main", "fs_main", LAYOUT, target, variant,
            )?;
            let maps = &run.key.maps;
            let maps = [
                maps.base_color,
                maps.metallic_roughness,
                maps.normal,
                maps.emissive,
                maps.occlusion,
            ];
            let mut bindings = vec![
                (0, globals),
                (1, objects),
                (2, Res::Buffer(run.key.mesh.vertices)),
                (3, Res::Buffer(run.key.mesh.indices)),
                (4, lights),
                (5, rows),
                (6, Res::Tex(run.key.texture.unwrap_or(unread))),
                (
                    7,
                    Res::Sampler(match run.key.texture {
                        Some(texture) if mipped.contains(&texture) => mipped_sampler,
                        _ => clamp,
                    }),
                ),
                (8, joints),
                (
                    9,
                    Res::Buffer(run.key.mesh.data.unwrap_or(run.key.mesh.vertices)),
                ),
                (20, shadow_views),
                (21, Res::Tex(shadow_map)),
                (22, Res::Sampler(shadow_sampler)),
                (23, environment_uniform),
                (24, Res::Tex(environment_map)),
                (26, Res::Sampler(environment_sampler)),
                (27, environment_brdf),
                (28, weights),
                (29, map_uvs),
            ];
            for (slot, map) in (0..).zip(maps) {
                let (texture, sampler) = match map {
                    Some(Map { texture, sampler }) => (
                        texture,
                        cx.rec
                            .sampler(sampler_key(&sampler), || sampler_descriptor(&sampler)),
                    ),
                    None => (unread, clamp),
                };
                bindings.push((10 + slot, Res::Tex(texture)));
                bindings.push((15 + slot, Res::Sampler(sampler)));
            }
            let load = if drawn == 0 {
                TargetLoad::Clear
            } else {
                TargetLoad::Load
            };
            cx.rec.draw_with_depth(
                program,
                &bindings,
                color,
                load,
                depth,
                load,
                run.vertices,
                1,
            );
        }

        if let Some(loaded) = environment.filter(|_| background) {
            let program = cx.render_program(
                "scene-3d-background",
                SHADER,
                "vs_background",
                "fs_background",
                BACKGROUND_LAYOUT,
                background_spec(),
                0,
            )?;
            let bindings = [
                (0, globals),
                (23, environment_uniform),
                (24, Res::Tex(loaded.reflections)),
                (
                    25,
                    Res::Tex(loaded.background.unwrap_or(loaded.reflections)),
                ),
                (26, Res::Sampler(environment_sampler)),
            ];
            let load = if draws.runs.is_empty() {
                TargetLoad::Clear
            } else {
                TargetLoad::Load
            };
            cx.rec.draw(program, &bindings, color, load, 3, 1, None);
        }

        let output = cx.storage_target(size);
        cx.spatial_pass(
            "scene-3d-resolve",
            RESOLVE_SHADER,
            color,
            &ResolveParams {
                supersample,
                _pad: [0; 3],
            },
            output,
            size,
        )?;
        Ok(Raster {
            tex: output,
            domain: TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}

/// The storage both of `render_3d`'s ways of drawing `draws` bind: the object table and the
/// tables it points into, and each run's uniform.
struct Tables {
    objects: Res,
    map_uvs: Res,
    weights: Res,
    joints: Res,
    runs: Vec<Res>,
}

impl Tables {
    fn new(
        cx: &mut FrameCx<'_>,
        scene: &Scene<GpuMesh, TexRef>,
        draws: &Draws<GpuMesh, TexRef>,
    ) -> Self {
        let mut map_uvs = Vec::new();
        let objects: Vec<GpuObject> = draws
            .rows
            .iter()
            .map(|row| gpu_object(&scene.objects[row.object], row, &mut map_uvs))
            .collect();
        let objects = cx.rec.storage_parts(&[bytemuck::cast_slice(&objects)]);
        // A storage binding can't be empty.
        if map_uvs.is_empty() {
            map_uvs.push(GpuMapUv::zeroed());
        }
        let map_uvs = cx.rec.storage_parts(&[bytemuck::cast_slice(&map_uvs)]);
        let mut weights = scene.weights.clone();
        if weights.is_empty() {
            weights.push(0.0);
        }
        let weights = cx.rec.storage_parts(&[bytemuck::cast_slice(&weights)]);
        let mut joints: Vec<[[f32; 4]; 4]> = scene
            .joints
            .iter()
            .map(|joint| joint.to_cols_array_2d())
            .collect();
        if joints.is_empty() {
            joints.push([[0.0; 4]; 4]);
        }
        let joints = cx.rec.storage_parts(&[bytemuck::cast_slice(&joints)]);
        let runs = draws
            .runs
            .iter()
            .map(|run| {
                cx.rec.uniform(&GpuRun {
                    first: run.first,
                    count: run.count,
                    base: run.base,
                    _pad: 0,
                })
            })
            .collect();
        Self {
            objects,
            map_uvs,
            weights,
            joints,
            runs,
        }
    }
}

fn clamp_sampler(cx: &mut FrameCx<'_>) -> lumen_gpu::SamplerRef {
    cx.rec.sampler(CLAMP_SAMPLER, || wgpu::SamplerDescriptor {
        label: Some("lumen 3d raster sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..wgpu::SamplerDescriptor::default()
    })
}

/// The distances along `camera`'s view that depth output maps to 0 and 1: `params`' own, or the
/// camera's near and far where they are 0.
fn depth_range(camera: &Camera, params: &super::Render3dParams) -> (f32, f32) {
    let pick = |value: f64, fallback: f32| if value > 0.0 { value as f32 } else { fallback };
    (
        pick(params.depth_near, camera.near),
        pick(params.depth_far, camera.far),
    )
}

/// Gives each raster that objects show smaller than it is a mip chain, in place of the raster in
/// the objects that show it, so that they are filtered rather than sampled sparsely. Returns the
/// textures it made.
fn mip_rasters(
    cx: &mut FrameCx<'_>,
    collected: &mut Collected,
    camera: &Camera,
    aspect: f32,
    target: Size,
) -> crate::Result<std::collections::HashSet<TexRef>> {
    use super::mips;
    if collected.rasters.is_empty() {
        return Ok(Default::default());
    }
    let view_proj = camera.projection_matrix(aspect) * camera.view();
    let rasters: std::collections::HashMap<TexRef, _> = collected
        .rasters
        .iter()
        .map(|&(node, raster)| (raster.tex, (node, raster)))
        .collect();
    if collected
        .scene
        .objects
        .iter()
        .any(|object| object.surface.texture.is_some() && Some(object.mesh) != collected.quad)
    {
        collected.place_bounds();
    }
    // The most texels of each raster any object shows in a pixel.
    let mut worst: std::collections::HashMap<TexRef, f32> = Default::default();
    for object in &collected.scene.objects {
        let Some(tex) = object.surface.texture else {
            continue;
        };
        let Some(&(_, raster)) = rasters.get(&tex) else {
            continue;
        };
        let size = raster.domain.storage_size;
        // A model's surface can stretch a raster across any shape, so a texel's footprint isn't
        // known from the object's bounds: its raster gets all its levels wherever it is in view.
        let shown = if Some(object.mesh) == collected.quad {
            mips::plane_texels_per_pixel(&object.world, &view_proj, size, target)
        } else {
            (mips::model_raster_is_mipped(size) && mips::bounds_in_view(&object.bounds, &view_proj))
                .then_some(f32::INFINITY)
        };
        if let Some(shown) = shown {
            let worst = worst.entry(tex).or_insert(shown);
            *worst = worst.max(shown);
        }
    }
    let mut made = std::collections::HashMap::new();
    for (node, raster) in collected.rasters.clone() {
        let Some(&shown) = worst.get(&raster.tex) else {
            continue;
        };
        let levels = mips::levels_for(raster.domain.storage_size, shown);
        if mips::is_minified(shown) && levels > 1 && !made.contains_key(&raster.tex) {
            made.insert(raster.tex, mips::mipped(cx, node, raster, levels)?);
        }
    }
    for object in &mut collected.scene.objects {
        if let Some(texture) = object.surface.texture.as_mut()
            && let Some(&mipped) = made.get(texture)
        {
            *texture = mipped;
        }
    }
    Ok(made.into_values().collect())
}

/// The environment `loaded` lights the scene with, if any, seen from `camera`.
fn gpu_environment(
    loaded: Option<&super::environment::LoadedEnvironment>,
    camera: &Camera,
    aspect: f32,
    params: &super::Render3dParams,
) -> GpuEnvironment {
    let view_proj = camera.projection_matrix(aspect) * camera.view();
    let (sin, cos) = (params.environment_rotation as f32).to_radians().sin_cos();
    GpuEnvironment {
        irradiance: loaded.map_or([[0.0; 4]; 9], |loaded| loaded.irradiance),
        clip_to_world: view_proj.inverse().to_cols_array_2d(),
        turn: [cos, sin],
        roughest: loaded.map_or(0.0, |loaded| loaded.levels.saturating_sub(1) as f32),
        background_blur: params.environment_background_blur.clamp(0.0, 1.0) as f32,
        map: u32::from(loaded.is_some()),
        _pad: [0; 3],
    }
}

fn globals(
    camera: &Camera,
    aspect: f32,
    light_count: u32,
    lighting: &Lighting,
    params: &super::Render3dParams,
    fog: [f32; 4],
    supersample: u32,
) -> GpuGlobals {
    let axis = |axis: lumen_3d::glam::Vec3| camera.world.transform_vector3(axis).normalize_or(axis);
    let (depth_near, depth_far) = depth_range(camera, params);
    GpuGlobals {
        view_proj: (camera.projection_matrix(aspect) * camera.view()).to_cols_array_2d(),
        camera_position: camera.position().into(),
        light_count,
        camera_back: camera.back().into(),
        orthographic: u32::from(matches!(camera.projection, Projection::Orthographic { .. })),
        ambient: lighting.ambient.into(),
        environment: params.environment.max(0.0) as f32,
        fog_color: [fog[0], fog[1], fog[2]],
        fog_density: params.fog_density.max(0.0) as f32,
        tone_mapping: params.tone_mapping as u32,
        exposure: (params.exposure as f32).exp2(),
        supersample,
        _pad: 0,
        camera_right: axis(lumen_3d::glam::Vec3::X).into(),
        depth_near,
        camera_up: axis(lumen_3d::glam::Vec3::Y).into(),
        depth_far,
    }
}
