//! Records `render_3d`: depth-tested draws of the scene's objects into a supersampled float
//! target, then a resolve into the node's raster.

use bytemuck::{Pod, Zeroable};
use lumen_3d::glam::Mat4;
use lumen_3d::{Camera, Draws, LightKind, Lighting, Object, Projection, Row};
use lumen_gpu::{
    DEPTH_FORMAT, DepthTest, LayoutEntry, LayoutKind, RenderTargetSpec, Res, Size, TargetLoad,
    TextureDomain, wgpu,
};

use super::Render3d;
use super::collect::{Collected, GpuMesh, solid_linear};
use crate::error::{LumenError, RenderError};
use crate::gpu::frame_graph::{FrameCx, Programs, Raster, RecordNode};
use crate::node::{DelegateEvalContext, DelegateEvaluable, required_input};

pub(crate) const SHADER: &str = include_str!("scene.wgsl");
const RESOLVE_SHADER: &str = include_str!("resolve.wgsl");
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
];

pub(crate) const FLAG_UNLIT: u32 = 1;
pub(crate) const FLAG_BLEND: u32 = 2;
pub(crate) const FLAG_MIRRORED: u32 = 4;
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
    _pad: [u32; 2],
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
    _pad: [f32; 3],
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
    _pad: u32,
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

fn spec(depth: DepthTest, cull: Option<wgpu::Face>) -> RenderTargetSpec {
    RenderTargetSpec {
        format: COLOR_FORMAT,
        blend: wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        depth: Some(depth),
        cull,
    }
}

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

fn gpu_object(object: &Object<GpuMesh>, row: &Row) -> GpuObject {
    let surface = &object.surface;
    let mut flags = 0;
    if surface.unlit {
        flags |= FLAG_UNLIT;
    }
    if surface.blending == lumen_3d::Blending::Blend {
        flags |= FLAG_BLEND;
    }
    if lumen_3d::mirrors(&object.world) {
        flags |= FLAG_MIRRORED;
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
        _pad: 0,
    }
}

fn gpu_light(light: &lumen_3d::Light) -> GpuLight {
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
        _pad: [0.0; 3],
    }
}

pub(super) fn render_error(node_id: crate::node::NodeId, details: String) -> LumenError {
    RenderError::NodeEvaluation {
        node_id,
        node_kind: "Render3d",
        details,
    }
    .into()
}

impl RecordNode for Render3d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
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
        collected.add(
            cx,
            required_input(self.id, "scene", self.scene)?,
            Mat4::IDENTITY,
        )?;
        let scene = collected.scene;
        let fog = solid_linear(self.id, "Render3d", &params.fog_color)?;
        let camera = scene.camera.unwrap_or_default();
        let view = camera.view();
        let draws = Draws::new(&scene.objects, &view)
            .map_err(|error| render_error(self.id, error.to_string()))?;
        if draws.runs.is_empty() {
            return Ok(cx.transparent(size));
        }

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

        let lighting = Lighting::of(&scene.lights);
        let mut lights: Vec<GpuLight> = lighting.direct.iter().map(gpu_light).collect();
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
        ));
        let lights = cx.rec.storage_parts(&[bytemuck::cast_slice(&lights)]);
        let objects: Vec<GpuObject> = draws
            .rows
            .iter()
            .map(|row| gpu_object(&scene.objects[row.object], row))
            .collect();
        let objects = cx.rec.storage_parts(&[bytemuck::cast_slice(&objects)]);

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
        for (drawn, run) in draws.runs.iter().enumerate() {
            let depth_test = if run.key.blend {
                DepthTest::Read
            } else {
                DepthTest::Write
            };
            let cull = match (run.key.double_sided, run.key.mirrored) {
                (true, _) => None,
                (false, false) => Some(wgpu::Face::Back),
                (false, true) => Some(wgpu::Face::Front),
            };
            let program = cx.render_program(
                "scene-3d",
                SHADER,
                "vs_main",
                "fs_main",
                LAYOUT,
                spec(depth_test, cull),
                0,
            )?;
            let rows = cx.rec.uniform(&GpuRun {
                first: run.first,
                count: run.count,
                base: run.base,
                _pad: 0,
            });
            let load = if drawn == 0 {
                TargetLoad::Clear
            } else {
                TargetLoad::Load
            };
            cx.rec.draw_with_depth(
                program,
                &[
                    (0, globals),
                    (1, objects),
                    (2, Res::Buffer(run.key.mesh.vertices)),
                    (3, Res::Buffer(run.key.mesh.indices)),
                    (4, lights),
                    (5, rows),
                ],
                color,
                load,
                depth,
                load,
                run.vertices,
                1,
            );
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

fn globals(
    camera: &Camera,
    aspect: f32,
    light_count: u32,
    lighting: &Lighting,
    params: &super::Render3dParams,
    fog: [f32; 4],
) -> GpuGlobals {
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
        _pad: [0; 2],
    }
}
