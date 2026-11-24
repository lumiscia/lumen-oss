//! Records `render_3d`: the scene's shadow maps, depth-tested draws of its objects into a
//! supersampled float target, and into targets beside it for their mattes when those are read,
//! the environment map behind them if it shows, then resolves into the node's rasters. Depth and
//! normal output draw the objects once at the output's size, unlit, then store what they drew.

use bytemuck::{Pod, Zeroable};
use lumen_3d::{
    Blending, Bounds, Camera, DrawKey, Draws, Filter, LightKind, Lighting, MAX_PROJECTORS, Map,
    MapUv, Object, Projection, Projector, Row, Sampler, ShadowAtlas, Wrap,
};
use lumen_gpu::{
    DEPTH_FORMAT, DepthTest, LayoutEntry, LayoutKind, RenderTargetSpec, Res, Size, TargetLoad,
    TexRef, TextureDomain, wgpu,
};

use super::collect::{Collected, CopySource, GpuMesh, solid_linear};
use super::{Light3d, Render3d, Render3dOutput, Render3dOutputDelegate};
use crate::error::{LumenError, RenderError};
use crate::gpu::frame_graph::{
    ComputeProgram, FrameCx, Programs, Raster, RecordNode, RenderProgram,
};
use crate::node::OutputPort;
use crate::node::{Deferred, DelegateEvalContext, DelegateEvaluable, required_input};
use std::borrow::Cow;

pub(crate) const SHADER: &str = include_str!("scene.wgsl");
const RESOLVE_SHADER: &str = include_str!("resolve.wgsl");
const DATA_SHADER: &str = include_str!("data.wgsl");
const MATTE_SHADER: &str = include_str!("matte.wgsl");
const LENS_SHADER: &str = include_str!("lens.wgsl");
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// What the views through the lens are summed in, so that adding many loses no precision.
const SUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// The most views `render_3d` draws through a lens.
const MAX_DOF_SAMPLES: u32 = 64;

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
    LayoutEntry::staged(30, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(31, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
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
    LayoutEntry::staged(30, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(31, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
];
/// What resolving the mattes binds: the first matte target, the params, the output and the second
/// target.
const MATTE_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::Texture),
    LayoutEntry::compute(1, LayoutKind::Uniform),
    LayoutEntry::compute(
        2,
        LayoutKind::StorageTexture(crate::gpu::WORKING_COLOR_FORMAT),
    ),
    LayoutEntry::compute(3, LayoutKind::Texture),
];
/// What adding a view through the lens to the sums binds: the view's colour or first matte target,
/// the params, its second matte target, the sum before it and the sum written.
const LENS_ADD_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::Texture),
    LayoutEntry::compute(1, LayoutKind::Uniform),
    LayoutEntry::compute(3, LayoutKind::Texture),
    LayoutEntry::compute(4, LayoutKind::UnfilterableTexture),
    LayoutEntry::compute(5, LayoutKind::StorageTexture(SUM_FORMAT)),
];
/// What adding the last view binds: the add's bindings, but for the output in place of the sum
/// written.
const LENS_LAST_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::Texture),
    LayoutEntry::compute(1, LayoutKind::Uniform),
    LayoutEntry::compute(
        2,
        LayoutKind::StorageTexture(crate::gpu::WORKING_COLOR_FORMAT),
    ),
    LayoutEntry::compute(3, LayoutKind::Texture),
    LayoutEntry::compute(4, LayoutKind::UnfilterableTexture),
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
    LayoutEntry::staged(30, wgpu::ShaderStages::VERTEX, LayoutKind::Storage),
    LayoutEntry::staged(31, wgpu::ShaderStages::FRAGMENT, LayoutKind::Storage),
];

const SCENE_3D: RenderProgram = RenderProgram {
    label: "scene-3d",
    shader: SHADER,
    vertex: "vs_main",
    fragment: "fs_main",
    layout: LAYOUT,
};
const SCENE_3D_MASKED: RenderProgram = RenderProgram {
    label: "scene-3d-masked",
    fragment: "fs_masked",
    ..SCENE_3D
};
/// The lit scene with up to three mattes in a second target, and with four in a third as well.
const SCENE_3D_MATTE: RenderProgram = RenderProgram {
    label: "scene-3d-matte",
    fragment: "fs_matte",
    ..SCENE_3D
};
const SCENE_3D_MATTES: RenderProgram = RenderProgram {
    label: "scene-3d-mattes",
    fragment: "fs_mattes",
    ..SCENE_3D
};
const SCENE_3D_MASKED_MATTE: RenderProgram = RenderProgram {
    label: "scene-3d-masked-matte",
    fragment: "fs_masked_matte",
    ..SCENE_3D
};
const SCENE_3D_MASKED_MATTES: RenderProgram = RenderProgram {
    label: "scene-3d-masked-mattes",
    fragment: "fs_masked_mattes",
    ..SCENE_3D
};
const SCENE_3D_SHADOW: RenderProgram = RenderProgram {
    label: "scene-3d-shadow",
    shader: SHADER,
    vertex: "vs_shadow",
    fragment: "fs_shadow",
    layout: SHADOW_LAYOUT,
};
const SCENE_3D_BACKGROUND: RenderProgram = RenderProgram {
    label: "scene-3d-background",
    shader: SHADER,
    vertex: "vs_background",
    fragment: "fs_background",
    layout: BACKGROUND_LAYOUT,
};
const SCENE_3D_DEPTH: RenderProgram = RenderProgram {
    label: "scene-3d-depth",
    shader: SHADER,
    vertex: "vs_main",
    fragment: "fs_depth",
    layout: DATA_LAYOUT,
};
const SCENE_3D_NORMAL: RenderProgram = RenderProgram {
    label: "scene-3d-normal",
    shader: SHADER,
    vertex: "vs_main",
    fragment: "fs_normal",
    layout: DATA_LAYOUT,
};
const SCENE_3D_DATA: ComputeProgram = ComputeProgram::spatial("scene-3d-data", DATA_SHADER);
const SCENE_3D_RESOLVE: ComputeProgram =
    ComputeProgram::spatial("scene-3d-resolve", RESOLVE_SHADER);
const SCENE_3D_MATTE_RESOLVE: ComputeProgram = ComputeProgram {
    label: "scene-3d-matte-resolve",
    shader: MATTE_SHADER,
    entry: "cs_main",
    layout: MATTE_LAYOUT,
};
/// Adding a view through the lens to the sums, and adding the last into the output, for colour and
/// for mattes.
const LENS_COLOR: [ComputeProgram; 2] = [
    ComputeProgram {
        label: "scene-3d-lens-color-add",
        shader: LENS_SHADER,
        entry: "cs_color_add",
        layout: LENS_ADD_LAYOUT,
    },
    ComputeProgram {
        label: "scene-3d-lens-color-last",
        shader: LENS_SHADER,
        entry: "cs_color_last",
        layout: LENS_LAST_LAYOUT,
    },
];
const LENS_MATTE: [ComputeProgram; 2] = [
    ComputeProgram {
        label: "scene-3d-lens-matte-add",
        entry: "cs_matte_add",
        ..LENS_COLOR[0]
    },
    ComputeProgram {
        label: "scene-3d-lens-matte-last",
        entry: "cs_matte_last",
        ..LENS_COLOR[1]
    },
];

const CLAMP_SAMPLER: u64 = 0x3d00_0001;
const NEAREST_SAMPLER: u64 = 0x3d00_0006;
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
/// The object draws only its `part` of the textured quad.
pub(crate) const FLAG_PART: u32 = 65536;
/// The object's raster is projected from its row of the projector table.
pub(crate) const FLAG_PROJECTED: u32 = 131072;
pub(crate) const LIGHT_DIRECTIONAL: u32 = 0;
pub(crate) const LIGHT_POINT: u32 = 1;
pub(crate) const LIGHT_SPOT: u32 = 2;

/// Keep in sync with `Globals` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Globals", source = SHADER)]
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
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Environment", source = SHADER)]
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
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Light", source = SHADER)]
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
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "ShadowView", source = SHADER)]
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
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Object", source = SHADER)]
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
    pub(crate) unlit_shadows: f32,
    /// The channel of the mattes the object's coverage goes to, or [`NO_MATTE`].
    pub(crate) matte: u32,
    /// The object's row of the projector table, with [`FLAG_PROJECTED`]; 0 otherwise.
    pub(crate) projector: u32,
    _pad: [u32; 3],
    /// [`Object::part`]'s edges, or the whole quad's.
    pub(crate) part: [f32; 4],
}

/// One row of the projector table; keep in sync with `Projector` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Projector", source = SHADER)]
pub(crate) struct GpuProjector {
    /// World to the projector's clip space, whose X and Y span its texture from -1 to 1.
    pub(crate) view_proj: [[f32; 4]; 4],
    pub(crate) position: [f32; 3],
    pub(crate) orthographic: u32,
    /// The projector's +Z axis in world space, back towards it.
    pub(crate) back: [f32; 3],
    /// The projector's view in the shadow views, or -1 for a projector without occlusion.
    pub(crate) shadow: i32,
}

/// One map's row of the map coordinates table; keep in sync with `MapUv` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "MapUv", source = SHADER)]
pub(crate) struct GpuMapUv {
    /// The transform's first row, then the set of texture coordinates it reads.
    pub(crate) first: [f32; 4],
    pub(crate) second: [f32; 4],
}

/// Keep in sync with `Run` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Run", source = SHADER)]
pub(crate) struct GpuRun {
    pub(crate) first: u32,
    pub(crate) count: u32,
    pub(crate) base: u32,
    /// Where the run's copies start in the instance buffer, or [`NO_INSTANCES`] for a run of
    /// objects drawn once.
    pub(crate) first_instance: u32,
    /// What places a run's copies.
    pub(crate) parent: [[f32; 4]; 4],
    /// The shadow views each object or copy is drawn into, one draw instance each.
    pub(crate) views: u32,
    _pad: [u32; 3],
}

pub(crate) const NO_INSTANCES: u32 = u32::MAX;
pub(crate) const NO_MATTE: u32 = u32::MAX;

/// One copy's row of an instance buffer, 64 bytes; keep in sync with `Instance` in `scene.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Instance", source = SHADER)]
pub(crate) struct GpuInstance {
    /// The rows of the copy's placement, which is affine.
    pub(crate) rows: [[f32; 4]; 3],
    /// The linear colour that multiplies the copy's base colour and emissive light; white leaves
    /// them as they are. The shader ignores its alpha.
    pub(crate) tint: [f32; 4],
}

/// The rows of `placements`' instance buffer.
fn gpu_instances(placements: &super::collect::Placements) -> Vec<GpuInstance> {
    placements
        .matrices()
        .iter()
        .enumerate()
        .map(|(index, placement)| {
            let rows = placement.transpose();
            let [r, g, b] = placements.tint(index);
            GpuInstance {
                rows: [rows.x_axis, rows.y_axis, rows.z_axis].map(Into::into),
                tint: [r, g, b, 1.0],
            }
        })
        .collect()
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Params", source = RESOLVE_SHADER)]
struct ResolveParams {
    supersample: u32,
    _pad: [u32; 3],
}

/// Keep in sync with `Params` in `matte.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Params", source = MATTE_SHADER)]
struct MatteParams {
    supersample: u32,
    second: u32,
    _pad: [u32; 2],
}

/// Keep in sync with `Params` in `lens.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Params", source = LENS_SHADER)]
struct LensParams {
    supersample: u32,
    second: u32,
    first: u32,
    weight: f32,
}

/// Keep in sync with `Params` in `data.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Params", source = DATA_SHADER)]
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
        extra_targets: 0,
    }
}

/// The programs drawing the lit scene and its masked surfaces with `names` mattes, and the matte
/// targets they draw them into beside the colour: one for up to three names, two for four.
fn lit_programs(names: usize) -> (&'static RenderProgram, &'static RenderProgram, u32) {
    match names {
        0 => (&SCENE_3D, &SCENE_3D_MASKED, 0),
        1..=3 => (&SCENE_3D_MATTE, &SCENE_3D_MASKED_MATTE, 1),
        _ => (&SCENE_3D_MATTES, &SCENE_3D_MASKED_MATTES, 2),
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

/// The program drawing depth or normal output, and what the raster holds where nothing is drawn.
type DataProgram = (&'static RenderProgram, [f32; 4]);

/// How `output` is drawn, unless it is drawn lit, as colour and mattes are.
fn data_program(output: Render3dOutput) -> Option<DataProgram> {
    match output {
        Render3dOutput::Color | Render3dOutput::Matte => None,
        Render3dOutput::Depth => Some((&SCENE_3D_DEPTH, [1.0; 4])),
        Render3dOutput::Normal => Some((&SCENE_3D_NORMAL, [0.0; 4])),
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
/// `map_uvs` when any moves its coordinates or reads the second set, and pointing at its row of
/// the projector table, which holds `projectors`.
fn gpu_object(
    object: &Object<GpuMesh, TexRef>,
    row: &Row,
    map_uvs: &mut Vec<GpuMapUv>,
    projectors: &[Projector],
) -> GpuObject {
    let surface = &object.surface;
    let mut flags = 0;
    let projector = surface
        .projector
        .and_then(|projector| projectors.iter().position(|&listed| listed == projector));
    if projector.is_some() {
        flags |= FLAG_PROJECTED;
    }
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
    if object.mirrored() {
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
    if object.part.is_some() {
        flags |= FLAG_PART;
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
        unlit_shadows: surface.unlit_shadows,
        matte: object.matte.map_or(NO_MATTE, u32::from),
        projector: projector.unwrap_or(0) as u32,
        _pad: [0; 3],
        part: object.part.unwrap_or([0.0, 0.0, 1.0, 1.0]),
    }
}

/// The distinct projectors of the objects `draws` draws, in the order they are first drawn.
fn projectors(
    objects: &[Object<GpuMesh, TexRef>],
    draws: &Draws<GpuMesh, TexRef>,
) -> Result<Vec<Projector>, String> {
    let mut projectors: Vec<Projector> = Vec::new();
    let projected = draws
        .rows
        .iter()
        .filter_map(|row| objects[row.object].surface.projector);
    for projector in projected {
        if projectors.contains(&projector) {
            continue;
        }
        if projectors.len() == MAX_PROJECTORS {
            return Err(format!(
                "the scene's materials project textures from more than {MAX_PROJECTORS} \
                 projectors"
            ));
        }
        projectors.push(projector);
    }
    Ok(projectors)
}

/// `projector`'s row of the projector table, whose occlusion reads shadow view `shadow`.
fn gpu_projector(projector: &Projector, shadow: Option<u32>) -> GpuProjector {
    let camera = &projector.camera;
    GpuProjector {
        view_proj: (camera.projection_matrix(projector.aspect) * camera.view()).to_cols_array_2d(),
        position: camera.position().into(),
        orthographic: u32::from(matches!(camera.projection, Projection::Orthographic { .. })),
        back: camera.back().into(),
        shadow: shadow.map_or(-1, |view| view as i32),
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

/// The shadow views of `lights` and `projectors` over the objects `draws` draws, when any of them
/// casts shadows.
fn shadow_atlas(
    objects: &[Object<GpuMesh, TexRef>],
    draws: &Draws<GpuMesh, TexRef>,
    lights: &[lumen_3d::Light],
    projectors: &[Projector],
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
        if (surface.receive_shadows && (!surface.unlit || surface.unlit_shadows > 0.0))
            || surface.shadow_catcher
        {
            receivers = receivers.union(object.bounds);
        }
    }
    ShadowAtlas::plan(
        lights, projectors, camera, aspect, casters, receivers, max_side,
    )
}

pub(super) fn render_error(node_id: crate::node::NodeId, details: String) -> LumenError {
    RenderError::evaluating::<Render3d>(node_id, details).into()
}

/// Whether `flag` may be on at some frame.
fn may(flag: &Deferred<bool>) -> bool {
    !matches!(flag, Deferred::Value(false))
}

/// Builds the program drawing shadow maps, when `light` may cast shadows.
pub(crate) fn light_programs(light: &Light3d, programs: &mut Programs<'_>) -> crate::Result<()> {
    if may(&light.params.cast_shadows) {
        programs.render(
            &SCENE_3D_SHADOW,
            RenderTargetSpec::depth_only(DepthTest::Write, None),
            0,
        )?;
    }
    Ok(())
}

/// Builds the programs summing the views through `camera`'s lens, when its `f_stop` may be above 0.
pub(crate) fn lens_programs(
    camera: &super::Camera3d,
    programs: &mut Programs<'_>,
) -> crate::Result<()> {
    if !matches!(camera.params.f_stop, Deferred::Value(f_stop) if f_stop <= 0.0) {
        for program in LENS_COLOR.iter().chain(&LENS_MATTE) {
            programs.compute(program)?;
        }
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
            programs.render(&SCENE_3D, catcher_spec(cull), CATCHER_VARIANT)?;
        }
    }
    Ok(())
}

impl Render3d {
    /// What the default `output` is: the output the `output` param selects.
    fn default_output(&self) -> Render3dOutput {
        match self.params.output {
            Render3dOutputDelegate::Depth => Render3dOutput::Depth,
            Render3dOutputDelegate::Normal => Render3dOutput::Normal,
            Render3dOutputDelegate::Matte => Render3dOutput::Matte,
            _ => Render3dOutput::Color,
        }
    }

    /// Draws `data`, depth or normals, of `collected`'s objects at `size` with `program`, from
    /// [`data_program`].
    fn record_data(
        &self,
        cx: &mut FrameCx<'_>,
        params: &super::Render3dParams,
        collected: &Collected,
        size: Size,
        data: Render3dOutput,
        (data_program, background): DataProgram,
    ) -> crate::Result<Raster> {
        let scene = &collected.scene;
        let camera = scene.camera.unwrap_or_default();
        if data == Render3dOutput::Depth {
            let (near, far) =
                depth_range(&camera, params).map_err(|details| render_error(self.id, details))?;
            if far <= near {
                return Err(render_error(
                    self.id,
                    format!("depth_far ({far}) must be farther than depth_near ({near})"),
                ));
            }
        }
        let draws = Draws::new(&scene.objects, &camera.view())
            .map_err(|error| render_error(self.id, error.to_string()))?;
        let output = cx.storage_target(size);
        let unread = cx.transparent(size).tex;
        let data_params = DataParams {
            background,
            drawn: u32::from(!draws.runs.is_empty()),
            _pad: [0; 3],
        };
        if draws.runs.is_empty() {
            if data == Render3dOutput::Normal {
                return Ok(cx.transparent(size));
            }
            cx.spatial_pass(&SCENE_3D_DATA, unread, &data_params, output, size)?;
            return Ok(Raster {
                tex: output,
                domain: TextureDomain::full_frame(size),
                bounds: None,
                reduced: None,
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
        let projectors =
            projectors(&scene.objects, &draws).map_err(|details| render_error(self.id, details))?;
        let tables = Tables::new(cx, collected, &draws, 1, (&projectors, None))?;
        let target = cx.rec.acquire(
            size,
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = cx
            .rec
            .acquire(size, DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let clamp = clamp_sampler(cx);
        for (drawn, (run, &(rows, instances))) in draws.runs.iter().zip(&tables.runs).enumerate() {
            let program = cx.render_program(data_program, data_spec(cull(&run.key)), 0)?;
            let mut bindings = vec![
                (30, instances),
                (0, globals),
                (1, tables.objects),
                (2, Res::Buffer(run.key.mesh.vertices)),
                (3, Res::Buffer(run.key.mesh.indices)),
                (5, rows),
                (6, Res::Tex(run.key.texture.unwrap_or(unread))),
                (7, Res::Sampler(plain_raster_sampler(cx, &run.key, clamp))),
                (8, tables.joints),
                (
                    9,
                    Res::Buffer(run.key.mesh.data.unwrap_or(run.key.mesh.vertices)),
                ),
                (28, tables.weights),
                (29, tables.map_uvs),
                (31, tables.projectors),
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
                run.instances,
            );
        }
        cx.spatial_pass(&SCENE_3D_DATA, target, &data_params, output, size)?;
        Ok(Raster {
            tex: output,
            domain: TextureDomain::full_frame(size),
            bounds: None,
            reduced: None,
        })
    }

    /// Whether some frame may draw the environment map behind the scene.
    fn may_show_background(&self) -> bool {
        let params = &self.params;
        !matches!(params.environment_background, Deferred::Value(false))
            && !matches!(&params.environment_map, Deferred::Value(map) if map.is_empty())
    }

    /// Warns that the shadow atlas had no room for the lights it dropped, naming each light's node
    /// once.
    fn warn_dropped_shadows(
        &self,
        cx: &mut FrameCx<'_>,
        collected: &Collected,
        atlas: &ShadowAtlas,
    ) {
        let scene = &collected.scene;
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
        cx.warn(
            self.id,
            crate::error::WarningKind::ShadowLimit { lights: dropped },
        );
    }
}

impl RecordNode for Render3d {
    /// Builds the programs scenes draw with, and the background's if the scene may show one, but
    /// for those drawing shadow maps and catchers, which the lights and surfaces asking for them
    /// build. Masked surfaces build their programs when first drawn.
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        let reads: Vec<_> = Render3dOutput::of_ports(programs.reads()).collect();
        // Colour and mattes, read together or alone, draw from one lit draw.
        let mattes = match reads.contains(&Render3dOutput::Matte) {
            true => self.params.mattes.0.len(),
            false => 0,
        };
        let mut lit = false;
        for output in reads {
            if let Some((data_program, _)) = data_program(output) {
                for cull in [Some(wgpu::Face::Back), Some(wgpu::Face::Front), None] {
                    programs.render(data_program, data_spec(cull), 0)?;
                }
                programs.compute(&SCENE_3D_DATA)?;
                continue;
            }
            if std::mem::replace(&mut lit, true) {
                continue;
            }
            let (shader, _, extra_targets) = lit_programs(mattes);
            for (depth, cull) in VARIANTS {
                let target = RenderTargetSpec {
                    extra_targets,
                    ..spec(depth, cull)
                };
                programs.render(shader, target, 0)?;
            }
            crate::gpu::mips::programs(programs)?;
            if self.may_show_background() {
                programs.render(&SCENE_3D_BACKGROUND, background_spec(), 0)?;
            }
            programs.compute(&SCENE_3D_RESOLVE)?;
            if mattes > 0 {
                programs.compute(&SCENE_3D_MATTE_RESOLVE)?;
            }
        }
        Ok(())
    }

    /// Reads of `output` read what the `output` param selects.
    fn canonical_port(&self, port: OutputPort) -> OutputPort {
        match port.index() {
            None => self.default_output().port(),
            Some(_) => port,
        }
    }

    fn record(&self, cx: &mut FrameCx<'_>) -> crate::Result<Raster> {
        self.record_output(cx, OutputPort::DEFAULT)
    }

    /// Draws `port`, from a walk of the scene the other outputs the graph reads share.
    fn record_output(&self, cx: &mut FrameCx<'_>, port: OutputPort) -> crate::Result<Raster> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
            pixel_scale: cx.pixel_scale(),
        })?;
        let canvas = cx.canvas_size();
        let pick = |value: u32, fallback: u32| match cx.explicit_extent(value) {
            0 => fallback,
            value => value,
        };
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
        if let Some(problem) = params.mattes.problem() {
            return Err(render_error(self.id, problem));
        }
        let scene = required_input(self.id, "scene", self.scene)?.node;
        let mattes = std::sync::Arc::new(params.mattes.clone());
        let walk = |cx: &mut FrameCx<'_>| {
            let mut collected = Collected::new(self.id, cx.scene_id(), mattes);
            collected.add(cx, scene)?;
            #[cfg(feature = "test-hooks")]
            cx.count_scene_walk();
            let composition = cx.composition();
            cx.capture_scene(self.id, || collected.captured(composition));
            if let Some(camera) = cx.camera_override() {
                collected.scene.camera = Some(super::collect::override_camera(&camera));
            }
            Ok(collected)
        };
        let reads: Vec<_> = Render3dOutput::of_ports(cx.outputs_read(self)).collect();
        // Outputs the graph reads more than one of share a walk of the scene.
        let shared;
        let collected = if reads.len() > 1 {
            shared = cx.shared_scene_walk(self.id, walk)?;
            Cow::Borrowed(&*shared)
        } else {
            Cow::Owned(walk(cx)?)
        };
        let output = Render3dOutput::of_port(port).unwrap_or(self.default_output());
        if let Some(program) = data_program(output) {
            return self.record_data(cx, &params, &collected, size, output, program);
        }
        if output == Render3dOutput::Matte {
            for name in collected.unused_mattes() {
                let name = name.to_string();
                cx.warn(self.id, crate::error::WarningKind::MatteUnused { name });
            }
        }
        // Colour read with the mattes draws them both at once.
        let both = [Render3dOutput::Color, Render3dOutput::Matte]
            .iter()
            .all(|lit| reads.contains(lit));
        let mattes = match both || output == Render3dOutput::Matte {
            true => params.mattes.0.len(),
            false => 0,
        };
        let draw = |cx: &mut FrameCx<'_>| {
            self.record_lit(cx, &params, collected.into_owned(), size, mattes)
        };
        let (color, matte) = match both {
            true => cx.shared_scene_draw(self.id, draw)?,
            false => draw(cx)?,
        };
        Ok(match output {
            Render3dOutput::Matte => matte,
            _ => color,
        })
    }
}

impl Render3d {
    /// Draws the lit scene `collected` holds at `size`, with the mattes of its first `mattes`
    /// matte names beside it, and returns the colour and the mattes, transparent for none.
    fn record_lit(
        &self,
        cx: &mut FrameCx<'_>,
        params: &super::Render3dParams,
        mut collected: Collected,
        size: Size,
        mattes: usize,
    ) -> crate::Result<(Raster, Raster)> {
        let max = cx.rec.device().limits().max_texture_dimension_2d;
        let scene = &collected.scene;
        if scene.lights.iter().any(|light| light.shadow.is_some())
            || scene.objects.iter().any(|object| {
                object
                    .surface
                    .projector
                    .is_some_and(|projector| projector.occlusion)
            })
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
        let fog = solid_linear::<Render3d>(self.id, &params.fog_color)?;
        let view = camera.view();
        let draws = Draws::new(&scene.objects, &view)
            .map_err(|error| render_error(self.id, error.to_string()))?;
        if draws.runs.is_empty() && !background {
            return Ok((cx.transparent(size), cx.transparent(size)));
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
        let projectors =
            projectors(&scene.objects, &draws).map_err(|details| render_error(self.id, details))?;
        let atlas = shadow_atlas(
            &scene.objects,
            &draws,
            &lighting.direct,
            &projectors,
            &camera,
            aspect,
            max.min(lumen_3d::atlas_side(cx.pixel_scale())),
        );
        if let Some(atlas) = &atlas
            && !atlas.dropped.is_empty()
        {
            self.warn_dropped_shadows(cx, &collected, atlas);
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
        let centre = globals(
            &camera,
            aspect,
            light_count,
            &lighting,
            params,
            fog,
            supersample,
        );
        let globals = cx.rec.uniform(&centre);
        let lights = cx.rec.storage_parts(&[bytemuck::cast_slice(&lights)]);
        let views = atlas.as_ref().map_or(1, |atlas| atlas.views.len() as u32);
        let tables = Tables::new(cx, &collected, &draws, views, (&projectors, atlas.as_ref()))?;

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
        let (lit, masked, extra_targets) = lit_programs(mattes);
        let mut targets = vec![color];
        for _ in 0..extra_targets {
            targets.push(cx.rec.acquire(
                target_size,
                COLOR_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ));
        }
        let clamp = clamp_sampler(cx);
        let mipped_sampler = cx
            .rec
            .sampler(super::mips::RASTER_SAMPLER, super::mips::raster_sampler);
        // Objects without a raster or a map still bind a texture and sampler in its place, which
        // their shader doesn't read, as meshes without skins bind their vertices.
        let unread = cx.transparent(size).tex;
        let Shadows {
            views: shadow_views,
            map: shadow_map,
            sampler: shadow_sampler,
        } = shadow_pass(cx, atlas.as_ref(), &draws, &tables, unread, clamp)?;

        let environment_bindings = bind_environment(
            cx,
            environment.as_ref(),
            &gpu_environment(
                environment.as_ref(),
                camera.projection_matrix(aspect) * camera.view(),
                params,
            ),
            unread,
            lights,
        );
        let EnvironmentBindings {
            uniform: environment_uniform,
            map: environment_map,
            sampler: environment_sampler,
            brdf: environment_brdf,
        } = environment_bindings;
        let Tables {
            objects,
            map_uvs,
            weights,
            joints,
            projectors,
            runs,
        } = tables;

        let lens = lens_samples(&camera, params.dof_samples);
        // A pinhole records each run as it is built; a lens keeps them to record once per view.
        let mut lit_draws = Vec::with_capacity(if lens.is_empty() { 0 } else { draws.runs.len() });
        for (drawn, (run, &(rows, instances))) in draws.runs.iter().zip(&runs).enumerate() {
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
            let target = RenderTargetSpec {
                extra_targets,
                ..target
            };
            let shader = if run.key.masked && !run.key.shadow_catcher {
                masked
            } else {
                lit
            };
            let program = cx.render_program(shader, target, variant)?;
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
                    Res::Sampler(if run.key.nearest {
                        nearest_sampler(cx)
                    } else if run
                        .key
                        .texture
                        .is_some_and(|texture| mipped.contains(&texture))
                    {
                        mipped_sampler
                    } else {
                        clamp
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
                (30, instances),
                (31, projectors),
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
            let draw = LitDraw {
                program,
                bindings,
                vertices: run.vertices,
                instances: run.instances,
            };
            if lens.is_empty() {
                record_lit(cx, &draw, drawn, &targets, depth);
            } else {
                lit_draws.push(draw);
            }
        }
        let background = environment.as_ref().filter(|_| background);
        if lens.is_empty() {
            if let Some(loaded) = background {
                background_pass(cx, loaded, globals, &environment_bindings, color, &draws)?;
            }
            let color = resolve(cx, color, supersample, size)?;
            let matte = match targets[1..] {
                [first, ref second @ ..] if !draws.runs.is_empty() => {
                    resolve_mattes(cx, first, second.first().copied(), supersample, size)?
                }
                _ => cx.transparent(size),
            };
            return Ok((color, matte));
        }

        // Each view through the lens is drawn into the same targets and added to the sums.
        let drawn_mattes = !draws.runs.is_empty() && targets.len() > 1;
        let sums = LensSums::new(cx, size, drawn_mattes);
        let mut outputs = None;
        for (index, &offset) in lens.iter().enumerate() {
            let view_proj = camera.lens_view_projection(aspect, offset);
            let globals = cx.rec.uniform(&GpuGlobals {
                view_proj: view_proj.to_cols_array_2d(),
                camera_position: camera.lens_position(offset).into(),
                ..centre
            });
            let environment_uniform =
                cx.rec
                    .uniform(&gpu_environment(environment.as_ref(), view_proj, params));
            draw_lit(
                cx,
                &mut lit_draws,
                (globals, environment_uniform),
                &targets,
                depth,
            );
            if let Some(loaded) = background {
                let environment = EnvironmentBindings {
                    uniform: environment_uniform,
                    ..environment_bindings
                };
                background_pass(cx, loaded, globals, &environment, color, &draws)?;
            }
            outputs = sums.add(cx, &targets, supersample, (index, lens.len()))?;
        }
        Ok(outputs.unwrap_or_else(|| (cx.transparent(size), cx.transparent(size))))
    }
}

/// A run of the lit scene ready to draw, but for the globals and environment uniforms each view
/// binds.
struct LitDraw {
    program: lumen_gpu::ProgramRef,
    bindings: Vec<(u32, Res)>,
    vertices: u32,
    instances: u32,
}

/// Draws `lit_draws` with `globals` and `environment` uniforms into `targets`, depth tested
/// against `depth`, clearing both first.
fn draw_lit(
    cx: &mut FrameCx<'_>,
    lit_draws: &mut [LitDraw],
    (globals, environment): (Res, Res),
    targets: &[TexRef],
    depth: TexRef,
) {
    for (drawn, draw) in lit_draws.iter_mut().enumerate() {
        for (slot, res) in &mut draw.bindings {
            match slot {
                0 => *res = globals,
                23 => *res = environment,
                _ => {}
            }
        }
        record_lit(cx, draw, drawn, targets, depth);
    }
}

/// Records the run `draw`, the one numbered `drawn` of the scene, which clears the targets and
/// depth when it is the first.
fn record_lit(
    cx: &mut FrameCx<'_>,
    draw: &LitDraw,
    drawn: usize,
    targets: &[TexRef],
    depth: TexRef,
) {
    let load = if drawn == 0 {
        TargetLoad::Clear
    } else {
        TargetLoad::Load
    };
    cx.rec.draw_targets_with_depth(
        draw.program,
        &draw.bindings,
        targets,
        load,
        depth,
        load,
        draw.vertices,
        draw.instances,
    );
}

/// The places on `camera`'s lens that `render_3d` draws `samples` views from for its depth of
/// field, or none when it draws one view, through a pinhole.
fn lens_samples(camera: &Camera, samples: u32) -> Vec<lumen_3d::glam::Vec2> {
    let lensed = camera.lens.aperture > 0.0
        && matches!(camera.projection, Projection::Perspective { .. })
        && samples > 1;
    match lensed {
        true => camera.lens.samples(samples.min(MAX_DOF_SAMPLES)),
        false => Vec::new(),
    }
}

/// The sums of the views through the lens drawn so far, averaged down to the output: the colour's,
/// and the mattes' when they are drawn. Each pair takes turns holding the sum read and the sum
/// written.
struct LensSums {
    size: Size,
    color: [TexRef; 2],
    mattes: Option<[TexRef; 2]>,
}

impl LensSums {
    fn new(cx: &mut FrameCx<'_>, size: Size, mattes: bool) -> Self {
        let mut pair = || {
            [(); 2].map(|_| {
                cx.rec.acquire(
                    size,
                    SUM_FORMAT,
                    lumen_gpu::TextureDesc::storage(size, SUM_FORMAT).usage,
                )
            })
        };
        let color = pair();
        let mattes = mattes.then(pair);
        Self {
            size,
            color,
            mattes,
        }
    }

    /// Adds the view numbered `index` of `count`, drawn into `targets`, colour first and then the
    /// matte targets. The last view returns the colour and matte outputs.
    fn add(
        &self,
        cx: &mut FrameCx<'_>,
        targets: &[TexRef],
        supersample: u32,
        (index, count): (usize, usize),
    ) -> crate::Result<Option<(Raster, Raster)>> {
        let view = SummedView {
            params: cx.rec.uniform(&LensParams {
                supersample,
                second: u32::from(targets.len() > 2),
                first: u32::from(index == 0),
                weight: 1.0 / count as f32,
            }),
            index,
            last: index + 1 == count,
            size: self.size,
        };
        let color = view.add(cx, &LENS_COLOR, self.color, (targets[0], targets[0]))?;
        let matte = match (self.mattes, &targets[1..]) {
            (Some(sums), &[first, ref second @ ..]) => {
                let second = second.first().copied().unwrap_or(first);
                view.add(cx, &LENS_MATTE, sums, (first, second))?
            }
            _ => None,
        };
        Ok(color.map(|color| {
            let matte = matte.unwrap_or_else(|| cx.transparent(self.size));
            (color, matte)
        }))
    }
}

/// One view through the lens being added to the sums.
struct SummedView {
    params: Res,
    index: usize,
    last: bool,
    size: Size,
}

impl SummedView {
    /// Adds the view, drawn into `sample` and the `second` matte target, to `sums` with the add
    /// and last of `programs`, and returns the output on the last view.
    fn add(
        &self,
        cx: &mut FrameCx<'_>,
        programs: &[ComputeProgram; 2],
        sums: [TexRef; 2],
        (sample, second): (TexRef, TexRef),
    ) -> crate::Result<Option<Raster>> {
        // The first view reads no sum, so it binds the sample in its place.
        let previous = match self.index {
            0 => sample,
            index => sums[(index + 1) % 2],
        };
        let (program, store, target) = match self.last {
            true => (&programs[1], 2, cx.storage_target(self.size)),
            false => (&programs[0], 5, sums[self.index % 2]),
        };
        let program = cx.program(program)?;
        cx.dispatch(
            program,
            &[
                (0, Res::Tex(sample)),
                (1, self.params),
                (3, Res::Tex(second)),
                (4, Res::Tex(previous)),
                (store, Res::Tex(target)),
            ],
            crate::gpu::frame_graph::dispatch_for(self.size),
        );
        Ok(self.last.then(|| Raster {
            tex: target,
            domain: TextureDomain::full_frame(self.size),
            bounds: None,
            reduced: None,
        }))
    }
}

/// The shadow map the main pass samples, the views drawn into it, and its sampler.
#[derive(Clone, Copy)]
struct Shadows {
    views: Res,
    map: TexRef,
    sampler: lumen_gpu::SamplerRef,
}

/// Draws the shadow casters of `draws` into `atlas`'s views, or takes the empty shadow map when
/// no light casts one.
fn shadow_pass(
    cx: &mut FrameCx<'_>,
    atlas: Option<&ShadowAtlas>,
    draws: &Draws<GpuMesh, TexRef>,
    tables: &Tables,
    unread: TexRef,
    clamp: lumen_gpu::SamplerRef,
) -> crate::Result<Shadows> {
    let &Tables {
        objects,
        map_uvs,
        weights,
        joints,
        projectors,
        ref runs,
    } = tables;
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
                &SCENE_3D_SHADOW,
                RenderTargetSpec::depth_only(DepthTest::Write, None),
                0,
            )?;
            let views = atlas.views.len() as u32;
            let casting = draws.runs.iter().zip(runs);
            for (cast, (run, &(rows, instances))) in
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
                    (7, Res::Sampler(plain_raster_sampler(cx, &run.key, clamp))),
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
                    (30, instances),
                    (31, projectors),
                ];
                let load = if cast == 0 {
                    TargetLoad::Clear
                } else {
                    TargetLoad::Load
                };
                cx.rec.draw_depth(
                    program,
                    &bindings,
                    map,
                    load,
                    run.vertices,
                    views * run.instances,
                );
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
    Ok(Shadows {
        views: shadow_views,
        map: shadow_map,
        sampler: shadow_sampler,
    })
}

/// What the main and background passes bind of the environment.
#[derive(Clone, Copy)]
struct EnvironmentBindings {
    uniform: Res,
    map: TexRef,
    sampler: lumen_gpu::SamplerRef,
    brdf: Res,
}

/// Binds `environment`, seen as `gpu` describes, or `unread` and `lights` in place of its map
/// and table when there is none.
fn bind_environment(
    cx: &mut FrameCx<'_>,
    environment: Option<&super::environment::LoadedEnvironment>,
    gpu: &GpuEnvironment,
    unread: TexRef,
    lights: Res,
) -> EnvironmentBindings {
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
    let environment_uniform = cx.rec.uniform(gpu);
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
    EnvironmentBindings {
        uniform: environment_uniform,
        map: environment_map,
        sampler: environment_sampler,
        brdf: environment_brdf,
    }
}

/// Draws `loaded`'s background behind what the main pass drew into `color`.
fn background_pass(
    cx: &mut FrameCx<'_>,
    loaded: &super::environment::LoadedEnvironment,
    globals: Res,
    environment: &EnvironmentBindings,
    color: TexRef,
    draws: &Draws<GpuMesh, TexRef>,
) -> crate::Result<()> {
    let EnvironmentBindings {
        uniform: environment_uniform,
        sampler: environment_sampler,
        ..
    } = *environment;
    let program = cx.render_program(&SCENE_3D_BACKGROUND, background_spec(), 0)?;
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
    Ok(())
}

/// Resolves the supersampled `color` into the output at `size`.
fn resolve(
    cx: &mut FrameCx<'_>,
    color: TexRef,
    supersample: u32,
    size: Size,
) -> crate::Result<Raster> {
    let output = cx.storage_target(size);
    cx.spatial_pass(
        &SCENE_3D_RESOLVE,
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
        reduced: None,
    })
}

/// Resolves the supersampled matte targets, `first` and the `second` a fourth name draws into, into
/// the matte output at `size`.
fn resolve_mattes(
    cx: &mut FrameCx<'_>,
    first: TexRef,
    second: Option<TexRef>,
    supersample: u32,
    size: Size,
) -> crate::Result<Raster> {
    let output = cx.storage_target(size);
    let program = cx.program(&SCENE_3D_MATTE_RESOLVE)?;
    let uniform = cx.rec.uniform(&MatteParams {
        supersample,
        second: u32::from(second.is_some()),
        _pad: [0; 2],
    });
    cx.dispatch(
        program,
        &[
            (0, Res::Tex(first)),
            (1, uniform),
            (2, Res::Tex(output)),
            (3, Res::Tex(second.unwrap_or(first))),
        ],
        crate::gpu::frame_graph::dispatch_for(size),
    );
    Ok(Raster {
        tex: output,
        domain: TextureDomain::full_frame(size),
        bounds: None,
        reduced: None,
    })
}

/// The storage both of `render_3d`'s ways of drawing `draws` bind: the object table and the
/// tables it points into, and each run's uniform with the instance buffer it draws copies from.
struct Tables {
    objects: Res,
    map_uvs: Res,
    weights: Res,
    joints: Res,
    projectors: Res,
    runs: Vec<(Res, Res)>,
}

impl Tables {
    /// The tables of `collected`'s objects that `draws` draws, into `views` shadow views when
    /// they are drawn into the shadow atlas, with the table of `projectors`, which take their
    /// views in `atlas`.
    fn new(
        cx: &mut FrameCx<'_>,
        collected: &Collected,
        draws: &Draws<GpuMesh, TexRef>,
        views: u32,
        (projectors, atlas): (&[Projector], Option<&ShadowAtlas>),
    ) -> crate::Result<Self> {
        let scene = &collected.scene;
        let mut map_uvs = Vec::new();
        let objects: Vec<GpuObject> = draws
            .rows
            .iter()
            .map(|row| gpu_object(&scene.objects[row.object], row, &mut map_uvs, projectors))
            .collect();
        let objects = cx.rec.storage_parts(&[bytemuck::cast_slice(&objects)]);
        let mut projector_rows: Vec<GpuProjector> = projectors
            .iter()
            .enumerate()
            .map(|(index, projector)| {
                let view = atlas.and_then(|atlas| atlas.projector_view[index]);
                gpu_projector(projector, view)
            })
            .collect();
        if projector_rows.is_empty() {
            projector_rows.push(GpuProjector::zeroed());
        }
        let projectors = cx
            .rec
            .storage_parts(&[bytemuck::cast_slice(&projector_rows)]);
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
        let mut lists: Vec<Option<Res>> = vec![None; collected.lists.len()];
        let runs = draws
            .runs
            .iter()
            .map(|run| {
                let copies = scene.objects[draws.rows[run.first as usize].object].instanced;
                let instances = match copies {
                    // Runs drawn once read no copy, and bind the object table in place of one.
                    None => objects,
                    Some(copies) => match lists[copies.list as usize] {
                        Some(instances) => instances,
                        None => {
                            let instances =
                                instance_buffer(cx, &collected.lists[copies.list as usize])?;
                            *lists[copies.list as usize].insert(instances)
                        }
                    },
                };
                let uniform = cx.rec.uniform(&GpuRun {
                    first: run.first,
                    count: run.count,
                    base: run.base,
                    first_instance: copies.map_or(NO_INSTANCES, |copies| copies.first),
                    parent: copies
                        .map_or(lumen_3d::glam::Mat4::IDENTITY, |copies| copies.parent)
                        .to_cols_array_2d(),
                    views,
                    _pad: [0; 3],
                });
                Ok((uniform, instances))
            })
            .collect::<crate::Result<_>>()?;
        Ok(Self {
            objects,
            map_uvs,
            weights,
            joints,
            projectors,
            runs,
        })
    }
}

/// The instance buffer of `list`: kept on the GPU while frames draw the same placements, else
/// uploaded for this frame.
fn instance_buffer(cx: &mut FrameCx<'_>, list: &super::collect::CopyList) -> crate::Result<Res> {
    let placements = match &list.source {
        CopySource::Placements(placements) => placements,
        CopySource::Particles(system) => return super::collect::particle_buffer(cx, system),
    };
    if list.kept
        && let Some(buffer) = cx.copy_buffer(list.key, placements, || gpu_instances(placements))
    {
        return Ok(Res::Buffer(buffer));
    }
    let records = gpu_instances(placements);
    Ok(cx.rec.storage_parts(&[bytemuck::cast_slice(&records)]))
}

/// The sampler a run reads its raster with outside the colour pass: `clamp`, or the nearest
/// sampler for a raster shown unfiltered.
fn plain_raster_sampler(
    cx: &mut FrameCx<'_>,
    key: &DrawKey<GpuMesh, TexRef>,
    clamp: lumen_gpu::SamplerRef,
) -> lumen_gpu::SamplerRef {
    if key.nearest {
        nearest_sampler(cx)
    } else {
        clamp
    }
}

/// Reads only a raster's full-size level, at its nearest texel.
fn nearest_sampler(cx: &mut FrameCx<'_>) -> lumen_gpu::SamplerRef {
    cx.rec.sampler(NEAREST_SAMPLER, || wgpu::SamplerDescriptor {
        label: Some("lumen 3d nearest raster sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        lod_max_clamp: 0.0,
        ..wgpu::SamplerDescriptor::default()
    })
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
/// camera's near and far where they are 0. A distance that is negative, not a number or more than
/// a float holds is an error naming its param.
fn depth_range(camera: &Camera, params: &super::Render3dParams) -> Result<(f32, f32), String> {
    let pick = |name: &str, value: f64, fallback: f32| {
        if value == 0.0 {
            return Ok(fallback);
        }
        let distance = value as f32;
        if value > 0.0 && distance.is_finite() {
            Ok(distance)
        } else {
            Err(format!(
                "{name} ({value}) must be a distance of 0 or more that a float holds"
            ))
        }
    };
    Ok((
        pick("depth_near", params.depth_near, camera.near)?,
        pick("depth_far", params.depth_far, camera.far)?,
    ))
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
        let Some(tex) = object.surface.texture.filter(|_| !object.surface.nearest) else {
            continue;
        };
        let Some(&(_, raster)) = rasters.get(&tex) else {
            continue;
        };
        let size = raster.domain.storage_size;
        // A shape's or model's surface can stretch a raster across any form, so a texel's
        // footprint isn't known from the object's bounds: its raster gets all its levels wherever
        // it is in view.
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
            made.insert(
                raster.tex,
                crate::gpu::mips::mipped(cx, node, raster, None, levels)?,
            );
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

/// The environment `loaded` lights the scene with, if any, seen through `view_proj`, world to
/// clip space.
fn gpu_environment(
    loaded: Option<&super::environment::LoadedEnvironment>,
    view_proj: lumen_3d::glam::Mat4,
    params: &super::Render3dParams,
) -> GpuEnvironment {
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
    // Only depth output reads the range, and it has refused a bad one by now.
    let (depth_near, depth_far) = depth_range(camera, params).unwrap_or((camera.near, camera.far));
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
