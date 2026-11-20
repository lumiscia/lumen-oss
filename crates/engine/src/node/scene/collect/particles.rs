//! Collects a `particles_3d`: its object scene is walked once and drawn as instanced objects whose
//! placements a compute pass writes on the GPU, from the node's params, the time and its surface.
//! The surface is walked as any scene is, then sampled through tables of its meshes' triangles,
//! built once per mesh on the CPU and kept, and put together on the GPU once per surface.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use foldhash::HashMap;
use lumen_3d::glam::{Mat3, Mat4, Vec3, Vec4};
use lumen_3d::{Blending, Bounds, Instanced, Mesh, Orientation};
use lumen_gpu::{BufRef, Dispatch, LayoutEntry, LayoutKind, Recorder, Res};

use super::instance::{CopyList, CopySource, rebased};
use super::{Collected, Extent, MAX_SCENE_JOINTS, MAX_SCENE_WEIGHTS, SceneCache, pose};
use crate::composition::SceneId;
use crate::error::{LumenError, RenderError};
use crate::gpu::frame_graph::{ComputeProgram, FrameCx, Programs};
use crate::media::{ModelResolver, resolve_as};
use crate::node::scene::{Particles3d, Shape3dKind};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, required_input};

const SHADER: &str = include_str!("../particles.wgsl");
const LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::Uniform),
    LayoutEntry::compute(1, LayoutKind::Storage),
    LayoutEntry::compute(2, LayoutKind::Storage),
    LayoutEntry::compute(3, LayoutKind::StorageReadWrite),
];
const PARTICLES: ComputeProgram = ComputeProgram {
    label: "particles-3d",
    shader: SHADER,
    entry: "cs_main",
    layout: LAYOUT,
};
/// Invocations of a workgroup of `cs_main`.
const WORKGROUP: u32 = 64;
const FLAG_ALIGN: u32 = 1;
const FLAG_STILL: u32 = 2;
/// Frames a mesh's triangle table is kept without a surface sampling it, as reading a model's
/// again means reading its file.
const IDLE_FRAMES: u64 = 600;

/// Builds the program placing particles.
pub(crate) fn particles_programs(programs: &mut Programs<'_>) -> crate::Result<()> {
    programs.compute(&PARTICLES)
}

fn particles_error(node_id: NodeId, details: impl Into<String>) -> LumenError {
    RenderError::evaluating::<Particles3d>(node_id, details.into()).into()
}

/// Keep in sync with `Params` in `particles.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Params", source = SHADER)]
struct GpuParticles {
    count: u32,
    seed: u32,
    parts: u32,
    flags: u32,
    time: f32,
    rate: f32,
    lifetime: f32,
    speed: f32,
    gravity: [f32; 3],
    cos_spread: f32,
    scale: [f32; 2],
    scale_over_life: [f32; 2],
    jitter: f32,
    _pad: [u32; 3],
}

/// One object of a surface; keep in sync with `Part` in `particles.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Part", source = SHADER)]
struct GpuPart {
    rows: [[f32; 4]; 3],
    first: u32,
    count: u32,
    weight: f32,
    turn: f32,
}

/// Keep in sync with `Triangle` in `particles.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "Triangle", source = SHADER)]
struct GpuTriangle {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
}

/// What a mesh is, whatever buffers hold it: what scenes' caches upload it for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MeshName {
    Shape(Shape3dKind),
    Quad,
    /// A text mesh's id among those kept.
    Text(String),
    /// A solid mesh's id among those kept.
    Solid(String),
    /// A model's identity.
    Model(String),
}

/// A mesh and the range of its indices an object draws.
type MeshPart = (MeshName, u32, u32);

/// The triangles of a mesh part, in the mesh's space, each with its share of their area added to
/// those before it.
struct Triangles {
    triangles: Vec<GpuTriangle>,
    bounds: Bounds,
}

impl Triangles {
    fn new(positions: impl Fn(u32) -> Option<Vec3>, indices: &[u32]) -> Self {
        let mut corners = Vec::with_capacity(indices.len() / 3);
        let mut total = 0.0_f64;
        let mut bounds = Bounds::EMPTY;
        for [a, b, c] in indices.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (positions(*a), positions(*b), positions(*c)) else {
                continue;
            };
            let area = f64::from((b - a).cross(c - a).length()) * 0.5;
            if !area.is_finite() {
                continue;
            }
            total += area;
            bounds = bounds.union(Bounds::of([a, b, c]));
            corners.push((a, b, c, total));
        }
        let last = corners.len().saturating_sub(1);
        let triangles = corners
            .into_iter()
            .enumerate()
            .map(|(index, (a, b, c, running))| {
                let share = if index == last || total == 0.0 {
                    1.0
                } else {
                    (running / total) as f32
                };
                GpuTriangle {
                    a: a.extend(share).into(),
                    b: b.extend(0.0).into(),
                    c: c.extend(0.0).into(),
                }
            })
            .collect();
        Self { triangles, bounds }
    }

    fn of_mesh(mesh: &Mesh, first: u32, count: u32) -> Self {
        let indices = mesh
            .indices
            .get(first as usize..(first + count) as usize)
            .unwrap_or_default();
        Self::new(
            |index| {
                mesh.vertices
                    .get(index as usize)
                    .map(|vertex| Vec3::from(vertex.position))
            },
            indices,
        )
    }
}

struct KeptTriangles {
    triangles: Arc<Triangles>,
    last_used: u64,
}

/// An instance buffer particles are placed into, and what it holds once they are.
struct Output {
    buffer: BufRef,
    capacity: u32,
    holds: Option<(GpuParticles, Vec<GpuPart>)>,
    last_used: u64,
}

/// The world area of each object of a surface, and what placed them.
struct PlacedAreas {
    used: Vec<MeshPart>,
    /// For each object, the table of its mesh part among `used` and its world matrix's rows.
    placements: Vec<(usize, [[f32; 4]; 3])>,
    areas: Vec<f64>,
}

/// What one `particles_3d` in a graph keeps on the GPU.
#[derive(Default)]
struct NodeBuffers {
    areas: Option<PlacedAreas>,
    /// Its surface's mesh parts, one after another, with the buffer holding their triangles.
    surface: Option<(Vec<MeshPart>, BufRef)>,
    /// The buffers its particles were placed into, one for each time a frame collects it.
    outputs: Vec<Output>,
    /// The frame `used` counts outputs for.
    tick: u64,
    used: usize,
    last_used: u64,
}

/// What `particles_3d` nodes sample their surfaces from and place their particles into.
#[derive(Default)]
pub(crate) struct ParticleCache {
    meshes: HashMap<MeshPart, KeptTriangles>,
    nodes: HashMap<(SceneId, NodeId), NodeBuffers>,
    /// Buffers replaced in a frame that drew them, with that frame, to free once it is over.
    retired: Vec<(BufRef, u64)>,
}

impl ParticleCache {
    /// Frees what neither this frame nor the last one used, and triangle tables no surface has
    /// sampled for [`IDLE_FRAMES`].
    pub(crate) fn trim(&mut self, rec: &mut Recorder, tick: u64) {
        self.meshes
            .retain(|_, kept| kept.last_used.saturating_add(IDLE_FRAMES) >= tick);
        self.nodes.retain(|_, node| {
            let used = node.last_used.saturating_add(1) >= tick;
            if !used {
                forget_node(rec, node);
            }
            used
        });
        for node in self.nodes.values_mut() {
            node.outputs.retain(|output| {
                let used = output.last_used.saturating_add(1) >= tick;
                if !used {
                    rec.forget_buffer(output.buffer);
                }
                used
            });
        }
        self.retired.retain(|&(buffer, retired)| {
            let freed = retired < tick;
            if freed {
                rec.forget_buffer(buffer);
            }
            !freed
        });
    }

    pub(crate) fn release(&mut self, rec: &mut Recorder) {
        self.meshes.clear();
        for (_, node) in self.nodes.drain() {
            forget_node(rec, &node);
        }
        for (buffer, _) in self.retired.drain(..) {
            rec.forget_buffer(buffer);
        }
    }
}

fn forget_node(rec: &mut Recorder, node: &NodeBuffers) {
    for output in &node.outputs {
        rec.forget_buffer(output.buffer);
    }
    if let Some((_, buffer)) = &node.surface {
        rec.forget_buffer(*buffer);
    }
}

/// A `particles_3d`'s particles at a frame: what the compute pass places them from, and where
/// they can be.
pub(crate) struct ParticleSystem {
    node: NodeId,
    params: GpuParticles,
    parts: Vec<GpuPart>,
    /// The surface's triangles, unless particles are emitted from the origin.
    triangles: Option<BufRef>,
    /// Where particles' origins can be, in the node's space.
    reach: Bounds,
    /// The largest scale a particle can have.
    largest: f32,
}

impl ParticleSystem {
    pub(crate) fn count(&self) -> u32 {
        self.params.count
    }

    /// What the particles of something covering `one` cover together: anywhere a particle can be,
    /// widened by `one` turned every way at the largest scale.
    pub(crate) fn bounds(&self, one: Bounds) -> Bounds {
        if one.is_empty() || self.reach.is_empty() {
            return Bounds::EMPTY;
        }
        let radius = one
            .corners()
            .iter()
            .map(|corner| corner.length())
            .fold(0.0, f32::max)
            * self.largest;
        Bounds {
            min: self.reach.min - Vec3::splat(radius),
            max: self.reach.max + Vec3::splat(radius),
        }
    }
}

/// The instance buffer `system`'s particles are placed into at this point of the frame: one the
/// node placed them into before, when it holds them still, or one placed into now.
pub(crate) fn instance_buffer(cx: &mut FrameCx<'_>, system: &ParticleSystem) -> crate::Result<Res> {
    let program = cx.program(&PARTICLES)?;
    let (scope, tick, rec, _, cache) = cx.particle_cache();
    let node = cache.nodes.entry((scope, system.node)).or_default();
    node.last_used = tick;
    if node.tick != tick {
        node.tick = tick;
        node.used = 0;
    }
    let count = system.params.count;
    let index = node.used;
    node.used += 1;
    if index == node.outputs.len() || node.outputs[index].capacity < count {
        let bytes = u64::from(count) * size_of::<super::super::render::GpuInstance>() as u64;
        let buffer = rec
            .device()
            .create_buffer(&lumen_gpu::wgpu::BufferDescriptor {
                label: Some("lumen 3d particles"),
                size: bytes,
                usage: lumen_gpu::wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
        let output = Output {
            buffer: rec.adopt_buffer(Arc::new(buffer)),
            capacity: count,
            holds: None,
            last_used: tick,
        };
        // Outputs are taken in order within a frame, so the one replaced is from an earlier one.
        match node.outputs.get_mut(index) {
            Some(replaced) => rec.forget_buffer(std::mem::replace(replaced, output).buffer),
            None => node.outputs.push(output),
        }
    }
    let output = &mut node.outputs[index];
    output.last_used = tick;
    let holds = (system.params, system.parts.clone());
    if output.holds.as_ref() == Some(&holds) {
        return Ok(Res::Buffer(output.buffer));
    }
    let buffer = output.buffer;
    output.holds = Some(holds);
    let parts = if system.parts.is_empty() {
        rec.storage_parts(&[bytemuck::bytes_of(&GpuPart::zeroed())])
    } else {
        rec.storage_parts(&[bytemuck::cast_slice(&system.parts)])
    };
    let triangles = match system.triangles {
        Some(triangles) => Res::Buffer(triangles),
        None => rec.storage_parts(&[bytemuck::bytes_of(&GpuTriangle::zeroed())]),
    };
    let uniform = rec.uniform(&system.params);
    rec.compute(
        program,
        &[
            (0, uniform),
            (1, parts),
            (2, triangles),
            (3, Res::Buffer(buffer)),
        ],
        Dispatch {
            x: count.div_ceil(WORKGROUP),
            y: 1,
            z: 1,
        },
    );
    Ok(Res::Buffer(buffer))
}

/// What a surface emits particles from: the surface's objects, and where they can emit from.
struct Surface {
    parts: Vec<GpuPart>,
    triangles: BufRef,
    bounds: Bounds,
}

/// One object of a surface as it is placed in the node's space.
struct Placed {
    world: Mat4,
    mesh: super::GpuMesh,
    first_index: u32,
    index_count: u32,
}

/// The name of the mesh `mesh`, which this frame draws, among those `cache` holds.
fn mesh_name(cache: &SceneCache, mesh: &super::GpuMesh) -> Option<MeshName> {
    if let Some((&kind, _)) = cache.shapes.iter().find(|(_, (held, _))| held == mesh) {
        return Some(MeshName::Shape(kind));
    }
    if cache.quad.is_some_and(|(held, _)| held == *mesh) {
        return Some(MeshName::Quad);
    }
    if let Some((identity, _)) = cache
        .models
        .iter()
        .find(|(_, cached)| cached.model.mesh == *mesh)
    {
        return Some(MeshName::Model(identity.clone()));
    }
    if let Some(id) = cache.texts.id_of(mesh) {
        return Some(MeshName::Text(id.to_string()));
    }
    cache
        .solids
        .id_of(mesh)
        .map(|id| MeshName::Solid(id.to_string()))
}

/// The triangles of `part`, built from the mesh `cache` holds for it.
fn build_triangles(
    cx_media: Option<&dyn crate::media::MediaStore>,
    max_glyphs: usize,
    cache: &mut SceneCache,
    (name, first, count): &MeshPart,
) -> crate::Result<Triangles> {
    let mesh = match name {
        MeshName::Shape(kind) => Mesh::primitive(super::primitive(*kind)),
        MeshName::Quad => Mesh::textured_quad(),
        MeshName::Text(id) => match cache.texts.rebuild(cx_media, id, max_glyphs) {
            Some(mesh) => mesh?,
            None => Mesh::default(),
        },
        MeshName::Solid(id) => {
            return Ok(match cache.solids.cpu_mesh(id) {
                Some(mesh) => Triangles::of_mesh(mesh, *first, *count),
                None => Triangles::new(|_| None, &[]),
            });
        }
        MeshName::Model(identity) => {
            let Some(cached) = cache.models.get(identity) else {
                return Ok(Triangles::new(|_| None, &[]));
            };
            let media = cx_media.ok_or_else(|| RenderError::Internal {
                details: "a model is drawn without a media store".to_string(),
            })?;
            let (_, resolver) = resolve_as::<dyn ModelResolver>(media, &cached.source)?;
            let model = lumen_3d::model::Model::from_glb(&resolver.data()?).map_err(|error| {
                RenderError::Internal {
                    details: format!("model `{}` no longer reads: {error}", cached.source),
                }
            })?;
            let geometry = &model.geometry;
            let indices = geometry
                .indices
                .get(*first as usize..(first + count) as usize)
                .unwrap_or_default();
            return Ok(Triangles::new(
                |index| {
                    geometry
                        .vertices
                        .get(index as usize)
                        .map(|vertex| Vec3::from(vertex.position))
                },
                indices,
            ));
        }
    };
    Ok(Triangles::of_mesh(&mesh, *first, *count))
}

/// What takes the textured quad to the part of it that `Object::part`, `[left, top, right,
/// bottom]` in texture coordinates, shows.
fn part_of_quad([left, top, right, bottom]: [f32; 4]) -> Mat4 {
    Mat4::from_translation(Vec3::new(
        (left + right) * 0.5 - 0.5,
        (1.0 - top - bottom) * 0.5,
        0.0,
    )) * Mat4::from_scale(Vec3::new(right - left, bottom - top, 1.0))
}

/// The area of `table`'s triangles as `world` places them.
fn world_area(table: &Triangles, world: &Mat4) -> f64 {
    let linear = Mat3::from_mat4(*world);
    table
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] =
                [triangle.a, triangle.b, triangle.c].map(|corner| Vec4::from(corner).truncate());
            f64::from((linear * (b - a)).cross(linear * (c - a)).length()) * 0.5
        })
        .sum()
}

/// The rows of `matrix`'s affine part.
fn rows(matrix: &Mat4) -> [[f32; 4]; 3] {
    let rows = matrix.transpose();
    [rows.x_axis, rows.y_axis, rows.z_axis].map(Into::into)
}

impl Collected {
    /// Adds `node`'s object scene as particles, placed by `parent` and the node's pose. Its
    /// lights and camera are left out.
    pub(super) fn add_particles(
        &mut self,
        cx: &mut FrameCx<'_>,
        node: &Particles3d,
        parent: Mat4,
    ) -> crate::Result<()> {
        let (params, seconds) = {
            let expr = cx.expr();
            let ctx = DelegateEvalContext::new(node.id, &expr);
            let seconds = if expr.fps > 0.0 {
                expr.frame / f64::from(expr.fps)
            } else {
                0.0
            };
            (node.params.eval(&ctx)?, seconds)
        };
        let count = params.count.0;
        if count == 0 {
            return Ok(());
        }
        let object = required_input(node.id, "object", node.object)?;
        let mut inner = Collected::new(self.render_id);
        inner.reached = self.reached;
        inner.nesting = self.nesting + 1;
        inner.instance_room = self.instance_room.saturating_sub(self.drawn);
        inner.add(cx, object.node)?;
        self.reached = inner.reached;
        for object in &inner.scene.objects {
            let refused = if object.instanced.is_some() {
                "copies of an instance_3d of more than one copy, or another particles_3d"
            } else if object.surface.blending == Blending::Blend {
                "a blended surface"
            } else if object.surface.texture.is_some() {
                "a surface showing a raster"
            } else {
                continue;
            };
            return Err(particles_error(
                node.id,
                format!(
                    "the object holds {refused}, which particles can't draw: they are drawn at \
                     once, so their surfaces must be opaque or masked and show no raster"
                ),
            ));
        }
        if self.drawn + inner.drawn > self.instance_room {
            return Err(particles_error(
                node.id,
                format!(
                    "an object of {} objects makes a scene of more than {} objects",
                    inner.drawn,
                    lumen_3d::MAX_SCENE_NODES
                ),
            ));
        }
        if self.indices + inner.indices > u64::from(u32::MAX) {
            return Err(particles_error(
                node.id,
                "the object makes a scene with too many vertices to draw",
            ));
        }

        let surface = match node.surface {
            Some(surface) => match self.surface(cx, node.id, surface.node)? {
                Some(surface) => Some(surface),
                // A surface with no area emits nothing.
                None => return Ok(()),
            },
            None => None,
        };
        if inner.scene.objects.is_empty() {
            return Ok(());
        }

        let joint_base = self.scene.joints.len();
        if joint_base + inner.scene.joints.len() > MAX_SCENE_JOINTS {
            return Err(particles_error(
                node.id,
                format!(
                    "the scene's skinned models have more than {MAX_SCENE_JOINTS} joints together"
                ),
            ));
        }
        self.scene.joints.extend_from_slice(&inner.scene.joints);
        let weight_base = self.scene.weights.len();
        if weight_base + inner.scene.weights.len() > MAX_SCENE_WEIGHTS {
            return Err(particles_error(
                node.id,
                format!(
                    "the scene's morphed models have more than {MAX_SCENE_WEIGHTS} morph target \
                     weights together"
                ),
            ));
        }
        self.scene.weights.extend_from_slice(&inner.scene.weights);

        let system = system(node.id, &params, seconds, surface);
        let list = self.lists.len() as u32;
        self.lists.push(CopyList {
            key: (node.id, 0),
            source: CopySource::Particles(Arc::new(system)),
            kept: false,
        });
        let carried = self.skins.len();
        self.skins.extend(
            inner
                .skins
                .iter()
                .map(|skin| rebased(skin, Mat4::IDENTITY, joint_base)),
        );
        let parent = parent * pose(&params.pose).matrix();
        for (object, extent) in inner.scene.objects.into_iter().zip(inner.extents) {
            let mut object = object;
            object.first_joint = object.first_joint.map(|joint| joint + joint_base as u32);
            if let Some((_, weight)) = &mut object.morph {
                *weight += weight_base as u32;
            }
            object.instanced = Some(Instanced {
                list,
                first: 0,
                count,
                parent,
                orientation: Orientation::Upright,
            });
            let extent = match extent {
                Extent::Skin(skin) => Extent::Skin(carried + skin),
                local @ Extent::Local(_) => local,
            };
            self.push(object, extent);
        }
        Ok(())
    }

    /// The surface the scene `surface` makes for particles of `node_id`, or `None` when it has
    /// no area.
    fn surface(
        &mut self,
        cx: &mut FrameCx<'_>,
        node_id: NodeId,
        surface: NodeId,
    ) -> crate::Result<Option<Surface>> {
        let mut collected = Collected::new(self.render_id);
        collected.reached = self.reached;
        collected.nesting = self.nesting + 1;
        collected.add(cx, surface)?;
        self.reached = collected.reached;

        let mut placed = Vec::new();
        for object in &collected.scene.objects {
            if object.index_count == 0 {
                continue;
            }
            // The part of the quad an image plane shows, or the whole mesh.
            let shown = object.part.map_or(Mat4::IDENTITY, part_of_quad);
            let at = |world: Mat4| Placed {
                world: world * shown,
                mesh: object.mesh,
                first_index: object.first_index,
                index_count: object.index_count,
            };
            match object.instanced {
                None => placed.push(at(object.world)),
                Some(copies) => {
                    let CopySource::Placements(placements) =
                        &collected.lists[copies.list as usize].source
                    else {
                        return Err(particles_error(
                            node_id,
                            "particles can't be emitted from another particles_3d's particles",
                        ));
                    };
                    let copied =
                        &placements.matrices()[copies.first as usize..][..copies.count as usize];
                    placed.extend(
                        copied
                            .iter()
                            .map(|copy| at(copies.parent * *copy * object.world)),
                    );
                }
            }
        }

        let media = cx.media();
        let max_glyphs = cx.max_text_glyphs();
        let (scope, tick, rec, scenes, cache) = cx.particle_cache();
        // The mesh part each object samples, by its place among the surface's.
        let mut names: Vec<(super::GpuMesh, u32, u32, usize)> = Vec::new();
        let mut used: Vec<MeshPart> = Vec::new();
        let mut tables: Vec<Arc<Triangles>> = Vec::new();
        let mut of_placed = Vec::with_capacity(placed.len());
        for object in &placed {
            let part = (object.mesh, object.first_index, object.index_count);
            let found = names
                .iter()
                .find(|&&(mesh, first, count, _)| (mesh, first, count) == part)
                .map(|&(.., index)| index);
            let index = match found {
                Some(index) => index,
                None => {
                    let Some(name) = mesh_name(scenes, &object.mesh) else {
                        return Err(RenderError::Internal {
                            details: "a surface draws a mesh no scene cache holds".to_string(),
                        }
                        .into());
                    };
                    let key = (name, object.first_index, object.index_count);
                    let triangles = match cache.meshes.get_mut(&key) {
                        Some(kept) => {
                            kept.last_used = tick;
                            kept.triangles.clone()
                        }
                        None => {
                            let triangles =
                                Arc::new(build_triangles(media, max_glyphs, scenes, &key)?);
                            cache.meshes.insert(
                                key.clone(),
                                KeptTriangles {
                                    triangles: triangles.clone(),
                                    last_used: tick,
                                },
                            );
                            triangles
                        }
                    };
                    let index = used
                        .iter()
                        .position(|kept| *kept == key)
                        .unwrap_or_else(|| {
                            used.push(key);
                            tables.push(triangles);
                            used.len() - 1
                        });
                    names.push((part.0, part.1, part.2, index));
                    index
                }
            };
            of_placed.push(index);
        }

        let mut firsts = Vec::with_capacity(tables.len());
        let mut total = 0_u32;
        for table in &tables {
            firsts.push(total);
            total += table.triangles.len() as u32;
        }
        let node = cache.nodes.entry((scope, node_id)).or_default();
        node.last_used = tick;
        // The areas are summed again only when what the surface places has changed.
        let placements: Vec<(usize, [[f32; 4]; 3])> = placed
            .iter()
            .zip(&of_placed)
            .map(|(object, &index)| (index, rows(&object.world)))
            .collect();
        let areas = match &node.areas {
            Some(held) if held.used == used && held.placements == placements => held.areas.clone(),
            _ => {
                let areas: Vec<f64> = placed
                    .iter()
                    .zip(&of_placed)
                    .map(|(object, &index)| world_area(&tables[index], &object.world))
                    .collect();
                node.areas = Some(PlacedAreas {
                    used: used.clone(),
                    placements,
                    areas: areas.clone(),
                });
                areas
            }
        };
        let mut parts = Vec::with_capacity(placed.len());
        let mut bounds = Bounds::EMPTY;
        let mut area = 0.0_f64;
        for ((object, &index), &weight) in placed.iter().zip(&of_placed).zip(&areas) {
            let table = &tables[index];
            let determinant = Mat3::from_mat4(object.world).determinant();
            if !(weight.is_finite() && weight > 0.0) || table.triangles.is_empty() {
                continue;
            }
            area += weight;
            bounds = bounds.union(table.bounds.transformed(&object.world));
            parts.push(GpuPart {
                rows: rows(&object.world),
                first: firsts[index],
                count: table.triangles.len() as u32,
                weight: area as f32,
                turn: if determinant < 0.0 { -1.0 } else { 1.0 },
            });
        }
        if parts.is_empty() {
            return Ok(None);
        }
        for part in &mut parts {
            part.weight = (f64::from(part.weight) / area) as f32;
        }
        if let Some(last) = parts.last_mut() {
            last.weight = 1.0;
        }

        let triangles = match &node.surface {
            Some((held, buffer)) if *held == used => *buffer,
            _ => {
                let limits = rec.device().limits();
                let bytes = u64::from(total) * size_of::<GpuTriangle>() as u64;
                let bindable = limits
                    .max_storage_buffer_binding_size
                    .min(limits.max_buffer_size);
                if bytes > bindable {
                    return Err(particles_error(
                        node_id,
                        format!(
                            "the surface has {total} triangles, more than this device can bind \
                             at once"
                        ),
                    ));
                }
                let contents: Vec<GpuTriangle> = tables
                    .iter()
                    .flat_map(|table| table.triangles.iter().copied())
                    .collect();
                let buffer = super::storage_buffer(
                    rec,
                    "lumen 3d particle surface",
                    bytemuck::cast_slice(&contents),
                );
                if let Some((_, replaced)) = node.surface.replace((used, buffer)) {
                    cache.retired.push((replaced, tick));
                }
                // A new surface buffer can take the slot of an old one, so what the outputs
                // hold no longer says what they were placed from.
                for output in &mut node.outputs {
                    output.holds = None;
                }
                buffer
            }
        };
        Ok(Some(Surface {
            parts,
            triangles,
            bounds,
        }))
    }
}

/// `node_id`'s particles at `seconds` with `params`, emitted from `surface`, or from the origin
/// along +Y without one.
fn system(
    node_id: NodeId,
    params: &crate::node::scene::Particles3dParams,
    seconds: f64,
    surface: Option<Surface>,
) -> ParticleSystem {
    let count = params.count.0;
    let rate = params.rate.max(0.0);
    let lifetime = params.lifetime.max(0.0);
    let speed = params.speed.max(0.0);
    let gravity = super::vec3(params.gravity);
    let still = rate == 0.0 && lifetime == 0.0 && speed == 0.0 && gravity == Vec3::ZERO;
    let time = if still { 0.0 } else { seconds };
    let (start, end) = params.scale_over_life;
    let life = if lifetime > 0.0 {
        start.max(end)
    } else {
        start
    };
    let largest = (params.scale.0.max(params.scale.1).max(0.0) * life.max(0.0)) as f32;

    // A particle is born at 0 seconds or later, and lives no longer than its lifetime or until
    // it is born again.
    let mut oldest = if still { 0.0 } else { seconds.max(0.0) };
    if rate > 0.0 {
        oldest = oldest.min(f64::from(count) / rate);
    }
    if lifetime > 0.0 {
        oldest = oldest.min(lifetime);
    }
    let oldest = oldest as f32;
    let (parts, triangles, emitted) = match surface {
        Some(surface) => (surface.parts, Some(surface.triangles), surface.bounds),
        None => (
            Vec::new(),
            None,
            Bounds {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            },
        ),
    };
    let travel = speed as f32 * oldest;
    let fallen = gravity * (0.5 * oldest * oldest);
    let reach = Bounds {
        min: emitted.min - Vec3::splat(travel) + fallen.min(Vec3::ZERO),
        max: emitted.max + Vec3::splat(travel) + fallen.max(Vec3::ZERO),
    };
    let seed = params.seed as u64;
    let mut flags = 0;
    if params.align_to_normal {
        flags |= FLAG_ALIGN;
    }
    if still {
        flags |= FLAG_STILL;
    }
    ParticleSystem {
        node: node_id,
        params: GpuParticles {
            count,
            seed: (seed ^ (seed >> 32)) as u32,
            parts: parts.len() as u32,
            flags,
            time: time as f32,
            rate: rate as f32,
            lifetime: lifetime as f32,
            speed: speed as f32,
            gravity: gravity.into(),
            cos_spread: (params.spread.clamp(0.0, 180.0).to_radians().cos()) as f32,
            scale: [params.scale.0 as f32, params.scale.1 as f32],
            scale_over_life: [
                params.scale_over_life.0 as f32,
                params.scale_over_life.1 as f32,
            ],
            jitter: params.rotation_jitter.clamp(0.0, 180.0).to_radians() as f32,
            _pad: [0; 3],
        },
        parts,
        triangles,
        reach,
        largest,
    }
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::super::super::render::GpuInstance;
    use super::*;

    #[test]
    fn particles_write_the_instance_rows_the_scene_shader_reads() {
        crate::gpu::shader_layouts::assert_layout(
            SHADER,
            "Instance",
            size_of::<GpuInstance>(),
            &[
                ("rows", offset_of!(GpuInstance, rows)),
                ("tint", offset_of!(GpuInstance, tint)),
            ],
        );
    }

    #[test]
    fn triangles_share_their_area_in_order() {
        let positions = [
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 2.0, 0.0),
        ];
        let triangles = Triangles::new(
            |index| positions.get(index as usize).copied(),
            &[0, 1, 2, 0, 3, 4, 0, 0, 1],
        );
        let shares: Vec<f32> = triangles.triangles.iter().map(|t| t.a[3]).collect();
        assert_eq!(shares, [0.2, 1.0, 1.0]);
    }

    #[test]
    fn bounds_reach_as_far_as_particles_travel_and_turn() {
        let params = crate::node::scene::Particles3dParams {
            speed: 2.0,
            lifetime: 1.5,
            gravity: [0.0, -4.0, 0.0],
            scale: (0.5, 2.0),
            ..Default::default()
        };
        let system = system(NodeId::new(1), &params, 10.0, None);
        // 3 units of travel either way, 4.5 down, and a unit cube's corner, √0.75 out, at twice
        // its size.
        let corner = 0.75_f32.sqrt() * 2.0;
        let bounds = system.bounds(super::super::UNIT_BOUNDS);
        let expected = Bounds {
            min: Vec3::new(-3.0, -7.5, -3.0) - Vec3::splat(corner),
            max: Vec3::new(3.0, 3.0, 3.0) + Vec3::splat(corner),
        };
        assert!((bounds.min - expected.min).abs().max_element() < 1e-5);
        assert!((bounds.max - expected.max).abs().max_element() < 1e-5);
        // Before any has lived long, none has gone far.
        let early = system_at(&params, 0.5).bounds(super::super::UNIT_BOUNDS);
        assert!((early.max.y - (1.0 + corner)).abs() < 1e-5);
    }

    fn system_at(params: &crate::node::scene::Particles3dParams, seconds: f64) -> ParticleSystem {
        system(NodeId::new(1), params, seconds, None)
    }
}
