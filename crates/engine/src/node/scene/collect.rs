//! Evaluates a scene's nodes at a frame into what `render_3d` draws.

use foldhash::HashMap;
use lumen_3d::glam::{Mat4, Vec3};
use lumen_3d::{
    Blending, Camera, Light, LightKind, Mesh, Object, Primitive, Projection, Scene, Surface,
    Transform,
};
use lumen_gpu::{BufRef, Recorder, wgpu};

use super::{Light3dKind, Material3d, Placement3d, Pose3d, Projection3d, Shape3dKind};
use crate::error::{LumenError, RenderError};
use crate::gpu::frame_graph::FrameCx;
use crate::node::vector::paint::Paint;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeKind};

/// A mesh's vertices and indices on the GPU, which the scene shader pulls vertices from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GpuMesh {
    pub(crate) vertices: BufRef,
    pub(crate) indices: BufRef,
}

impl GpuMesh {
    fn upload(rec: &mut Recorder, mesh: &Mesh) -> Self {
        let mut buffer = |label, contents: &[u8]| {
            let buffer = wgpu::util::DeviceExt::create_buffer_init(
                rec.device(),
                &wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE,
                },
            );
            rec.adopt_buffer(std::sync::Arc::new(buffer))
        };
        Self {
            vertices: buffer("lumen 3d vertices", bytemuck::cast_slice(&mesh.vertices)),
            indices: buffer("lumen 3d indices", bytemuck::cast_slice(&mesh.indices)),
        }
    }
}

/// The primitives' meshes, uploaded once with how many indices each draws.
#[derive(Default)]
pub(crate) struct SceneCache {
    shapes: HashMap<Shape3dKind, (GpuMesh, u32)>,
}

impl SceneCache {
    pub(crate) fn release(&mut self, rec: &mut Recorder) {
        for (mesh, _) in self.shapes.values() {
            rec.forget_buffer(mesh.vertices);
            rec.forget_buffer(mesh.indices);
        }
        self.shapes.clear();
    }

    fn shape(&mut self, rec: &mut Recorder, kind: Shape3dKind) -> (GpuMesh, u32) {
        *self.shapes.entry(kind).or_insert_with(|| {
            let mesh = Mesh::primitive(match kind {
                Shape3dKind::Box => Primitive::Box,
                Shape3dKind::Sphere => Primitive::Sphere,
                Shape3dKind::Plane => Primitive::Plane,
                Shape3dKind::Cylinder => Primitive::Cylinder,
                Shape3dKind::Torus => Primitive::Torus,
                Shape3dKind::Cone => Primitive::Cone,
            });
            (GpuMesh::upload(rec, &mesh), mesh.indices.len() as u32)
        })
    }
}

fn vec3(value: [f64; 3]) -> Vec3 {
    Vec3::new(value[0] as f32, value[1] as f32, value[2] as f32)
}

fn pose(pose: &Pose3d) -> Transform {
    Transform {
        position: vec3(pose.position),
        rotation: vec3(pose.rotation),
        scale: Vec3::ONE,
    }
}

fn placement(placement: &Placement3d) -> Transform {
    Transform {
        scale: vec3(placement.scale),
        ..pose(&placement.pose)
    }
}

/// `paint` as linear colour with straight alpha.
pub(super) fn solid_linear(
    node_id: NodeId,
    kind: &'static str,
    paint: &Paint,
) -> crate::Result<[f32; 4]> {
    let Paint::SolidColor(r, g, b, a) = *paint else {
        return Err(RenderError::NodeEvaluation {
            node_id,
            node_kind: kind,
            details: "3D colours must be solid; a gradient can't light a surface".to_string(),
        }
        .into());
    };
    let [r, g, b] = [r, g, b].map(lumen_3d::srgb_to_linear);
    Ok([r, g, b, f32::from(a) / 255.0])
}

fn surface(node_id: NodeId, material: &Material3d) -> crate::Result<Surface> {
    let mut base_color = solid_linear(node_id, "Shape3d", &material.color)?;
    base_color[3] *= material.opacity.clamp(0.0, 1.0) as f32;
    let emissive = material.emissive.max(0.0) as f32;
    Ok(Surface {
        base_color,
        metallic: material.metallic.clamp(0.0, 1.0) as f32,
        roughness: material.roughness.clamp(0.0, 1.0) as f32,
        emissive: [0, 1, 2].map(|i| base_color[i] * emissive),
        blending: if base_color[3] < 1.0 {
            Blending::Blend
        } else {
            Blending::Opaque
        },
        unlit: material.unlit,
        double_sided: material.double_sided,
    })
}

/// Adds the scene `node_id` outputs, placed by `parent`, to `out`.
pub(super) fn collect(
    cx: &mut FrameCx<'_>,
    node_id: NodeId,
    parent: Mat4,
    out: &mut Scene<GpuMesh>,
) -> crate::Result<()> {
    let node = cx
        .node(node_id)
        .ok_or(RenderError::MissingNode { node_id })?;
    let expr = cx.expr();
    let ctx = DelegateEvalContext {
        node_id,
        expr: &expr,
    };
    match node {
        NodeKind::Merge3d(merge) => {
            let world = parent * placement(&merge.params.eval(&ctx)?.placement).matrix();
            for &object in &merge.objects {
                collect(cx, object, world, out)?;
            }
        }
        NodeKind::Shape3d(shape) => {
            let params = shape.params.eval(&ctx)?;
            let size = vec3(params.size).max(Vec3::ZERO);
            let world = parent * placement(&params.placement).matrix() * Mat4::from_scale(size);
            let surface = surface(node_id, &params.material)?;
            let (rec, cache) = cx.scene_cache();
            let (mesh, index_count) = cache.shape(rec, params.shape);
            out.objects.push(Object {
                world,
                mesh,
                index_count,
                surface,
                center: world.transform_point3(Vec3::ZERO),
            });
        }
        NodeKind::Camera3d(camera) => {
            let params = camera.params.eval(&ctx)?;
            let local = if params.look_at {
                lumen_3d::look_at(vec3(params.pose.position), vec3(params.target))
            } else {
                pose(&params.pose).matrix()
            };
            let near = (params.near as f32).max(1e-4);
            out.camera.get_or_insert(Camera {
                world: parent * local,
                projection: match params.projection {
                    Projection3d::Perspective => Projection::Perspective {
                        fov_y: (params.fov.clamp(1.0, 179.0) as f32).to_radians(),
                    },
                    Projection3d::Orthographic => Projection::Orthographic {
                        height: (params.ortho_height as f32).max(1e-4),
                    },
                },
                near,
                far: (params.far as f32).max(near + 1e-3),
            });
        }
        NodeKind::Light3d(light) => {
            let params = light.params.eval(&ctx)?;
            let [r, g, b, _] = solid_linear(node_id, "Light3d", &params.color)?;
            out.lights.push(Light::new(
                match params.light {
                    Light3dKind::Directional => LightKind::Directional,
                    Light3dKind::Point => LightKind::Point,
                    Light3dKind::Spot => LightKind::Spot,
                    Light3dKind::Ambient => LightKind::Ambient,
                },
                Vec3::new(r, g, b) * params.intensity as f32,
                parent * pose(&params.pose).matrix(),
                params.range as f32,
                params.spot_angle as f32,
                params.spot_softness as f32,
            ));
        }
        // Validation keeps rasters out of scene inputs.
        _ => {
            return Err(LumenError::from(RenderError::NodeEvaluation {
                node_id,
                node_kind: "Render3d",
                details: "a scene input takes scene nodes, not a raster".to_string(),
            }));
        }
    }
    Ok(())
}
