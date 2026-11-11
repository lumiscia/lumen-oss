//! Evaluates a scene's nodes at a frame into what `render_3d` draws.

mod instance;

use std::sync::Arc;

use foldhash::HashMap;
use lumen_3d::glam::{Mat4, Vec3};
use lumen_3d::model::{Material, Model, ModelError, ModelScene, Playback, SkinBounds, Texture};
use lumen_3d::{
    Blending, Bounds, Camera, Light, LightKind, MAX_SCENE_NODES, Mesh, Object, Primitive,
    Projection, Scene, Shadow, Surface, Transform,
};
use lumen_gpu::{BufRef, Recorder, TexRef, wgpu};

use super::{Light3dKind, Material3d, Model3d, Placement3d, Pose3d, Projection3d, Shape3dKind};
use crate::error::{LumenError, MediaError, MediaFailure, RenderError};
use crate::gpu::frame_graph::FrameCx;
use crate::media::{AssetKind, ModelResolver};
use crate::node::vector::paint::Paint;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeKind, required_input};

/// The bytes of models the cache keeps on the GPU beyond those the latest frame drew or a kept
/// output drew.
const MODEL_BUDGET_BYTES: u64 = 256 << 20;

/// The most joint matrices one frame's scene may hold.
const MAX_SCENE_JOINTS: usize = 1 << 16;
/// The shortest a directional light's shadows reach from the camera.
const MIN_SHADOW_DISTANCE: f32 = 0.01;

/// A mesh's vertices and indices on the GPU, which the scene shader pulls vertices from, and the
/// joints and weights of a skinned one's vertices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GpuMesh {
    pub(crate) vertices: BufRef,
    pub(crate) indices: BufRef,
    pub(crate) skin: Option<BufRef>,
}

pub(super) fn storage_buffer(rec: &mut Recorder, label: &str, contents: &[u8]) -> BufRef {
    let buffer = wgpu::util::DeviceExt::create_buffer_init(
        rec.device(),
        &wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage: wgpu::BufferUsages::STORAGE,
        },
    );
    rec.adopt_buffer(Arc::new(buffer))
}

impl GpuMesh {
    pub(super) fn upload(rec: &mut Recorder, mesh: &Mesh) -> Self {
        Self {
            vertices: storage_buffer(
                rec,
                "lumen 3d vertices",
                bytemuck::cast_slice(&mesh.vertices),
            ),
            indices: storage_buffer(rec, "lumen 3d indices", bytemuck::cast_slice(&mesh.indices)),
            skin: None,
        }
    }

    pub(super) fn forget(&self, rec: &mut Recorder) {
        for buffer in [self.vertices, self.indices].into_iter().chain(self.skin) {
            rec.forget_buffer(buffer);
        }
    }
}

/// A model on the GPU.
#[derive(Clone)]
struct LoadedModel {
    scene: Arc<ModelScene>,
    mesh: GpuMesh,
    /// One per texture of the model, in its order.
    textures: Arc<[TexRef]>,
    /// The bounds of each primitive's vertices, by its first index and index count.
    bounds: Arc<HashMap<(u32, u32), Bounds>>,
    /// Each skin's bounds before posing.
    joint_bounds: Arc<Vec<SkinBounds>>,
}

struct CachedModel {
    model: LoadedModel,
    bytes: u64,
    last_used: u64,
}

/// What scenes draw, uploaded once: the primitives' and the image plane's meshes with how many
/// indices each draws, and models by their resolver's ID.
#[derive(Default)]
pub(crate) struct SceneCache {
    shapes: HashMap<Shape3dKind, (GpuMesh, u32)>,
    quad: Option<(GpuMesh, u32)>,
    models: HashMap<String, CachedModel>,
    model_bytes: u64,
    /// A depth texture bound in place of a shadow atlas by scenes without shadows.
    no_shadows: Option<TexRef>,
    pub(super) environments: super::environment::EnvironmentCache,
    pub(super) texts: super::text_mesh::TextMeshes,
}

impl SceneCache {
    pub(crate) fn release(&mut self, rec: &mut Recorder) {
        for (mesh, _) in self.shapes.values().chain(&self.quad) {
            mesh.forget(rec);
        }
        self.shapes.clear();
        self.quad = None;
        if let Some(texture) = self.no_shadows.take() {
            rec.forget(texture);
        }
        for (_, cached) in self.models.drain() {
            forget_model(rec, &cached.model);
        }
        self.model_bytes = 0;
        self.environments.release(rec);
        self.texts.release(rec);
    }

    /// Frees environment maps as [`Self::trim`] frees models.
    pub(crate) fn trim_environments<'k>(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        kept: impl IntoIterator<Item = &'k String>,
    ) {
        self.environments
            .trim(rec, tick, &kept.into_iter().collect());
    }

    /// Frees models, least recently drawn first, while the cache holds more than
    /// [`MODEL_BUDGET_BYTES`], keeping those frame `tick` drew and those `kept` names, and frees
    /// text meshes the same way.
    pub(crate) fn trim<'k>(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        kept: impl IntoIterator<Item = &'k String>,
    ) {
        let kept: std::collections::HashSet<&String> = kept.into_iter().collect();
        self.texts.trim(rec, tick, &kept);
        if self.model_bytes <= MODEL_BUDGET_BYTES {
            return;
        }
        let freed = evictions(
            self.models.iter().map(|(id, cached)| {
                let protected = cached.last_used == tick || kept.contains(id);
                (id, cached.bytes, cached.last_used, protected)
            }),
            self.model_bytes,
            MODEL_BUDGET_BYTES,
        );
        let freed: Vec<String> = freed.into_iter().cloned().collect();
        for id in freed {
            if let Some(cached) = self.models.remove(&id) {
                forget_model(rec, &cached.model);
                self.model_bytes -= cached.bytes;
            }
        }
    }

    /// The model `resolver` serves, loaded once per resolver ID and marked drawn in `tick`.
    fn model(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        source: &str,
        resolver: &dyn ModelResolver,
    ) -> Result<LoadedModel, MediaError> {
        if let Some(cached) = self.models.get_mut(resolver.id()) {
            cached.last_used = tick;
            return Ok(cached.model.clone());
        }
        let bytes = resolver.data()?;
        let model =
            Model::from_glb(&bytes).map_err(|error| model_failure(source, &bytes, error))?;
        drop(bytes);
        let (loaded, size) = upload_model(rec, source, model)?;
        self.model_bytes += size;
        self.models.insert(
            resolver.id().to_string(),
            CachedModel {
                model: loaded.clone(),
                bytes: size,
                last_used: tick,
            },
        );
        Ok(loaded)
    }

    pub(super) fn no_shadows(&mut self, rec: &mut Recorder) -> TexRef {
        *self.no_shadows.get_or_insert_with(|| {
            let texture = rec.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("lumen 3d no shadows"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: lumen_gpu::DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            rec.adopt(Arc::new(texture))
        })
    }

    fn quad(&mut self, rec: &mut Recorder) -> (GpuMesh, u32) {
        *self.quad.get_or_insert_with(|| {
            let mesh = Mesh::textured_quad();
            (GpuMesh::upload(rec, &mesh), mesh.indices.len() as u32)
        })
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

/// Why `bytes`, the file `source`, is not a model that can be read.
fn model_failure(source: &str, bytes: &[u8], error: ModelError) -> MediaError {
    let wanted = [AssetKind::Model];
    let failure = match error {
        ModelError::Invalid(_) if !bytes.starts_with(b"glTF") => {
            match crate::media::sniff_kind(bytes) {
                Some(found) => MediaFailure::wrong_kind(
                    source,
                    &wanted,
                    Some(found),
                    "the file is not a binary glTF (.glb)",
                ),
                None if bytes.trim_ascii_start().starts_with(b"{") => MediaFailure::unsupported(
                    source,
                    "glTF JSON (.gltf) files are not read; use a binary glTF (.glb)",
                ),
                None => MediaFailure::decode(source, error.to_string()),
            }
        }
        ModelError::Invalid(details) => MediaFailure::decode(source, details),
        ModelError::Unsupported(details) => MediaFailure::unsupported(source, details),
        ModelError::TooLarge {
            details,
            limit,
            actual,
        } => MediaFailure::too_large(source, details, limit, actual),
    };
    failure.wanting(&wanted).into()
}

/// Which of `models`, each with its bytes, the tick it was last drawn in and whether it must stay,
/// to free so that the `total` bytes they hold come down to `budget`: least recently drawn first,
/// then by key, and none that must stay.
pub(super) fn evictions<K: Ord + Copy>(
    models: impl Iterator<Item = (K, u64, u64, bool)>,
    total: u64,
    budget: u64,
) -> Vec<K> {
    let mut idle: Vec<(u64, K, u64)> = models
        .filter(|&(_, _, _, protected)| !protected)
        .map(|(key, bytes, last_used, _)| (last_used, key, bytes))
        .collect();
    idle.sort_unstable();
    let mut total = total;
    let mut freed = Vec::new();
    for (_, key, bytes) in idle {
        if total <= budget {
            break;
        }
        total -= bytes;
        freed.push(key);
    }
    freed
}

fn forget_model(rec: &mut Recorder, model: &LoadedModel) {
    model.mesh.forget(rec);
    for &texture in model.textures.iter() {
        rec.forget(texture);
    }
}

/// Uploads `model`'s geometry and textures, returning them with the bytes they take.
fn upload_model(
    rec: &mut Recorder,
    source: &str,
    model: Model,
) -> Result<(LoadedModel, u64), MediaError> {
    let limits = rec.device().limits();
    let geometry = &model.geometry;
    let parts: [&[u8]; 3] = [
        bytemuck::cast_slice(&geometry.vertices),
        bytemuck::cast_slice(&geometry.indices),
        bytemuck::cast_slice(&geometry.skin),
    ];
    let largest = parts
        .iter()
        .map(|part| part.len() as u64)
        .max()
        .unwrap_or(0);
    let bindable = limits
        .max_storage_buffer_binding_size
        .min(limits.max_buffer_size);
    if largest > bindable {
        return Err(MediaFailure::too_large(
            source,
            "the model's geometry is larger than this device can bind at once",
            bindable,
            Some(largest),
        )
        .wanting(&[AssetKind::Model])
        .into());
    }
    let max_side = limits.max_texture_dimension_2d;
    if let Some(side) = model
        .textures
        .iter()
        .map(|texture| texture.width.max(texture.height))
        .filter(|&side| side > max_side)
        .max()
    {
        return Err(MediaFailure::too_large(
            source,
            format!(
                "an image in the model is larger than this device's {max_side}-pixel texture limit"
            ),
            u64::from(max_side),
            Some(u64::from(side)),
        )
        .wanting(&[AssetKind::Model])
        .into());
    }
    let bounds = primitive_bounds(&model);
    let joint_bounds = model.scene.joint_bounds(&model.geometry);
    let mut bytes = parts.iter().map(|part| part.len() as u64).sum::<u64>()
        + (bounds.len() * size_of::<((u32, u32), Bounds)>()) as u64
        + joint_bounds
            .iter()
            .map(|skin| ((skin.joints.len() + 1) * size_of::<Bounds>()) as u64)
            .sum::<u64>();
    let mesh = GpuMesh {
        vertices: storage_buffer(rec, "lumen 3d model vertices", parts[0]),
        indices: storage_buffer(rec, "lumen 3d model indices", parts[1]),
        skin: (!geometry.skin.is_empty())
            .then(|| storage_buffer(rec, "lumen 3d model skin", parts[2])),
    };
    let textures = model
        .textures
        .iter()
        .map(|texture| {
            bytes += texture
                .levels
                .iter()
                .map(|level| level.len() as u64)
                .sum::<u64>();
            upload_texture(rec, texture)
        })
        .collect();
    let loaded = LoadedModel {
        scene: Arc::new(model.scene),
        mesh,
        textures,
        bounds: Arc::new(bounds),
        joint_bounds: Arc::new(joint_bounds),
    };
    Ok((loaded, bytes))
}

/// The bounds of the vertices of each primitive `model` draws, by its first index and index count.
fn primitive_bounds(model: &Model) -> HashMap<(u32, u32), Bounds> {
    let geometry = &model.geometry;
    let mut bounds = HashMap::default();
    for posed in model.scene.pose(None).primitives {
        let primitive = posed.primitive;
        bounds
            .entry((primitive.first_index, primitive.index_count))
            .or_insert_with(|| {
                let start = primitive.first_index as usize;
                let indices = geometry
                    .indices
                    .get(start..start + primitive.index_count as usize)
                    .unwrap_or_default();
                Bounds::of(indices.iter().filter_map(|&index| {
                    let vertex = geometry.vertices.get(index as usize)?;
                    Some(Vec3::from(vertex.position))
                }))
            });
    }
    bounds
}

/// What a unit primitive covers, centred on the origin.
const UNIT_BOUNDS: Bounds = Bounds {
    min: Vec3::splat(-0.5),
    max: Vec3::splat(0.5),
};

fn upload_texture(rec: &mut Recorder, texture: &Texture) -> TexRef {
    let texture = wgpu::util::DeviceExt::create_texture_with_data(
        rec.device(),
        rec.queue(),
        &wgpu::TextureDescriptor {
            label: Some("lumen 3d model texture"),
            size: wgpu::Extent3d {
                width: texture.width,
                height: texture.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: texture.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if texture.srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &texture.levels.concat(),
    );
    rec.adopt(Arc::new(texture))
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

fn surface(
    node_id: NodeId,
    kind: &'static str,
    material: &Material3d,
) -> crate::Result<Surface<TexRef>> {
    let mut base_color = solid_linear(node_id, kind, &material.color)?;
    base_color[3] *= material.opacity.clamp(0.0, 1.0) as f32;
    let emissive = material.emissive.max(0.0) as f32;
    Ok(Surface {
        base_color,
        metallic: material.metallic.clamp(0.0, 1.0) as f32,
        roughness: material.roughness.clamp(0.0, 1.0) as f32,
        emissive: [0, 1, 2].map(|i| base_color[i] * emissive),
        // A catcher's alpha only scales how dark its shadows are.
        blending: if base_color[3] < 1.0 && !material.shadow_catcher {
            Blending::Blend
        } else {
            Blending::Opaque
        },
        unlit: material.unlit,
        double_sided: material.double_sided,
        cast_shadows: material.cast_shadows,
        receive_shadows: material.receive_shadows,
        shadow_catcher: material.shadow_catcher,
        ..Surface::default()
    })
}

/// How a model's `material` shades, reading the model's `textures`.
fn model_surface(
    material: &Material,
    textures: &[TexRef],
    flat_normals: bool,
    params: &super::Model3dParams,
) -> Surface<TexRef> {
    Surface {
        base_color: material.base_color,
        texture: None,
        // Reading checked that every map names one of the model's textures.
        maps: material.maps.map(|&texture| textures[texture]),
        metallic: material.metallic,
        roughness: material.roughness,
        emissive: material.emissive,
        normal_scale: material.normal_scale,
        occlusion_strength: material.occlusion_strength,
        blending: material.blending,
        unlit: material.unlit,
        double_sided: material.double_sided,
        flat_normals,
        cast_shadows: params.cast_shadows,
        receive_shadows: params.receive_shadows,
        shadow_catcher: false,
    }
}

fn model_error(node_id: NodeId, source: &str, details: impl std::fmt::Display) -> LumenError {
    RenderError::NodeEvaluation {
        node_id,
        node_kind: "Model3d",
        details: format!("model `{source}`: {details}"),
    }
    .into()
}

/// What an object covers before it is placed in the world.
enum Extent {
    /// Bounds its world matrix places.
    Local(Bounds),
    /// Within the bounds of one of the scene's posed skins.
    Skin(usize),
}

/// A skin posed in the scene: where its joint matrices start in the scene's joints, the model
/// matrix they are placed by, and its joints' bounds before posing.
struct PosedSkin {
    first_joint: usize,
    root: Mat4,
    joint_bounds: Arc<Vec<SkinBounds>>,
    skin: usize,
}

/// The scene a `render_3d` draws, as far as its nodes have been walked.
pub(super) struct Collected {
    pub(super) scene: Scene<GpuMesh, TexRef>,
    /// The node of each of the scene's lights.
    pub(super) light_nodes: Vec<NodeId>,
    /// One per object, from which [`Self::place_bounds`] works out its bounds.
    extents: Vec<Extent>,
    skins: Vec<PosedSkin>,
    render_id: NodeId,
    /// Scene nodes walked, counting a node each time it is reached.
    reached: usize,
    /// Indices the objects draw in all.
    indices: u64,
    /// How many `instance_3d` nodes hold the scene being collected.
    nesting: u32,
    /// The most objects an `instance_3d` may bring the scene to, which is what the nodes holding it
    /// leave room for once they have copied it.
    instance_room: usize,
}

impl Collected {
    pub(super) fn new(render_id: NodeId) -> Self {
        Self {
            scene: Scene::default(),
            light_nodes: Vec::new(),
            extents: Vec::new(),
            skins: Vec::new(),
            render_id,
            reached: 0,
            indices: 0,
            nesting: 0,
            instance_room: MAX_SCENE_NODES,
        }
    }

    fn push(&mut self, object: Object<GpuMesh, TexRef>, extent: Extent) {
        self.indices += u64::from(object.index_count);
        self.scene.objects.push(object);
        self.extents.push(extent);
    }

    /// Sets every object's bounds in the world, which only shadows need: each skin's from the
    /// joints that move its vertices, once for every object it moves.
    pub(super) fn place_bounds(&mut self) {
        let joints = &self.scene.joints;
        let skins: Vec<Bounds> = self
            .skins
            .iter()
            .map(|skin| {
                let Some(bounds) = skin.joint_bounds.get(skin.skin) else {
                    return Bounds::EMPTY;
                };
                let posed = joints.get(skin.first_joint..).unwrap_or_default();
                bounds
                    .joints
                    .iter()
                    .zip(posed)
                    .filter(|(bounds, _)| !bounds.is_empty())
                    .fold(
                        bounds.unmoved.transformed(&skin.root),
                        |all, (bounds, joint)| all.union(bounds.transformed(&(skin.root * *joint))),
                    )
            })
            .collect();
        for (object, extent) in self.scene.objects.iter_mut().zip(&self.extents) {
            object.bounds = match *extent {
                Extent::Local(bounds) => bounds.transformed(&object.world),
                Extent::Skin(skin) => skins[skin],
            };
        }
    }

    /// Adds the scene `node_id` outputs, walking its nodes depth first in input order. A scene
    /// that reaches more than [`MAX_SCENE_NODES`] nodes fails as soon as it does, before a node
    /// wired in many times over can expand much further.
    pub(super) fn add(&mut self, cx: &mut FrameCx<'_>, node_id: NodeId) -> crate::Result<()> {
        // Each node still to walk, with the transform its parents place it by.
        let mut pending = vec![(node_id, Mat4::IDENTITY)];
        while let Some((node_id, parent)) = pending.pop() {
            self.reached += 1;
            if self.reached > MAX_SCENE_NODES {
                return Err(super::render::render_error(
                    self.render_id,
                    format!(
                        "the scene expands to more than {MAX_SCENE_NODES} nodes, counting a \
                         node each time it is reached"
                    ),
                ));
            }
            if let Some(children) = self.add_node(cx, node_id, parent)? {
                pending.extend(children.into_iter().rev());
            }
        }
        Ok(())
    }

    /// Adds what `node_id`, placed by `parent`, outputs itself, and returns the nodes a group
    /// places, with the transform it places them by.
    fn add_node(
        &mut self,
        cx: &mut FrameCx<'_>,
        node_id: NodeId,
        parent: Mat4,
    ) -> crate::Result<Option<Vec<(NodeId, Mat4)>>> {
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
                return Ok(Some(
                    merge
                        .objects
                        .iter()
                        .map(|&object| (object, world))
                        .collect(),
                ));
            }
            NodeKind::Instance3d(instance) => self.add_instances(cx, instance, parent)?,
            NodeKind::Shape3d(shape) => {
                let params = shape.params.eval(&ctx)?;
                let size = vec3(params.size).max(Vec3::ZERO);
                let world = parent * placement(&params.placement).matrix() * Mat4::from_scale(size);
                let surface = surface(node_id, "Shape3d", &params.material)?;
                let (rec, cache) = cx.scene_cache();
                let (mesh, index_count) = cache.shape(rec, params.shape);
                self.push(
                    Object {
                        world,
                        mesh,
                        first_index: 0,
                        index_count,
                        surface,
                        center: world.transform_point3(Vec3::ZERO),
                        first_joint: None,
                        bounds: Bounds::EMPTY,
                    },
                    Extent::Local(UNIT_BOUNDS),
                );
            }
            NodeKind::Text3d(text) => {
                let params = text.params.eval(&ctx)?;
                let world = parent * placement(&params.placement).matrix();
                let surface = surface(node_id, "Text3d", &params.material)?;
                if let Some(text) = super::text_mesh::text_mesh(cx, node_id, &params)? {
                    let local = text.bounds;
                    self.push(
                        Object {
                            world,
                            mesh: text.mesh,
                            first_index: 0,
                            index_count: text.index_count,
                            surface,
                            center: world.transform_point3((local.min + local.max) * 0.5),
                            first_joint: None,
                            bounds: Bounds::EMPTY,
                        },
                        Extent::Local(local),
                    );
                }
            }
            NodeKind::ImagePlane3d(plane) => {
                let params = plane.params.eval(&ctx)?;
                let source = cx.input_native(required_input(node_id, "source", plane.source)?)?;
                let display = source.domain.display_size();
                let scale = lumen_3d::plane_scale(
                    params.width.max(0.0) as f32,
                    (display.width, display.height),
                );
                let world =
                    parent * placement(&params.placement).matrix() * Mat4::from_scale(scale);
                let mut surface = surface(node_id, "ImagePlane3d", &params.material)?;
                // A raster has transparent texels whatever its opacity, so a plane blends unless
                // told it has none. A shadow catcher keeps its alpha, which sets its coverage.
                surface.blending = if params.opaque && !params.material.shadow_catcher {
                    Blending::Opaque
                } else {
                    Blending::Blend
                };
                surface.texture = Some(source.tex);
                let (rec, cache) = cx.scene_cache();
                let (mesh, index_count) = cache.quad(rec);
                self.push(
                    Object {
                        world,
                        mesh,
                        first_index: 0,
                        index_count,
                        surface,
                        center: world.transform_point3(Vec3::ZERO),
                        first_joint: None,
                        bounds: Bounds::EMPTY,
                    },
                    Extent::Local(Bounds {
                        min: Vec3::new(-0.5, -0.5, 0.0),
                        max: Vec3::new(0.5, 0.5, 0.0),
                    }),
                );
            }
            NodeKind::Model3d(model) => {
                let params = model.params.eval(&ctx)?;
                let seconds = if expr.fps > 0.0 {
                    expr.frame / f64::from(expr.fps)
                } else {
                    0.0
                };
                self.add_model(cx, model, params, seconds, parent)?;
            }
            NodeKind::Camera3d(camera) => {
                let params = camera.params.eval(&ctx)?;
                let local = if params.look_at {
                    lumen_3d::look_at(vec3(params.pose.position), vec3(params.target))
                } else {
                    pose(&params.pose).matrix()
                };
                let near = (params.near as f32).max(1e-4);
                self.scene.camera.get_or_insert(Camera {
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
                let light = Light::new(
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
                );
                self.scene.lights.push(Light {
                    shadow: params.cast_shadows.then(|| Shadow {
                        softness: (params.shadow_softness as f32).max(0.0),
                        distance: (params.shadow_distance as f32).max(MIN_SHADOW_DISTANCE),
                    }),
                    ..light
                });
                self.light_nodes.push(node_id);
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
        Ok(None)
    }

    /// Adds the primitives of `node`'s model, posed `seconds` into the composition's clip.
    fn add_model(
        &mut self,
        cx: &mut FrameCx<'_>,
        node: &Model3d,
        params: super::Model3dParams,
        seconds: f64,
        parent: Mat4,
    ) -> crate::Result<()> {
        let source = params.source.as_str();
        let error = |details: String| model_error(node.id, source, details);
        let at = cx.node_param(node.id, "source");
        let in_node = |error: MediaError| {
            LumenError::from(error).media_used_as(&[AssetKind::Model], || at.clone())
        };
        let media = cx.media().ok_or_else(|| {
            in_node(MediaError::source_not_found(
                source,
                "this render has no media store",
            ))
        })?;
        let resolver = media.get_model_resolver(source).map_err(in_node)?;
        let tick = cx.tick();
        let (rec, cache) = cx.scene_cache();
        let model = cache
            .model(rec, tick, source, &*resolver)
            .map_err(in_node)?;
        cx.drew_model(resolver.id());
        let scene = &model.scene;
        let clip = scene.animation(&params.animation).map_err(error)?;
        let replaced = match (params.texture_material.as_str(), node.texture) {
            ("", None) => None,
            ("", Some(_)) => {
                return Err(error(
                    "a `texture` input needs `texture_material` to name the material it shows on"
                        .to_string(),
                ));
            }
            (name, texture) => {
                let material = scene.material(name).map_err(error)?;
                match texture {
                    Some(texture) => Some((material, cx.input_native(texture)?.tex)),
                    None => None,
                }
            }
        };
        if self.scene.objects.len() + scene.draw_count() > MAX_SCENE_NODES {
            return Err(super::render::render_error(
                self.render_id,
                format!("the scene draws more than {MAX_SCENE_NODES} objects"),
            ));
        }

        let playback = clip.map(|clip| Playback {
            clip,
            seconds: (seconds * params.animation_speed + params.animation_offset) as f32,
            looped: params.animation_loop,
        });
        let pose = scene.pose(playback);
        let first_joint = self.scene.joints.len();
        if first_joint + pose.joints.len() > MAX_SCENE_JOINTS {
            return Err(super::render::render_error(
                self.render_id,
                format!(
                    "the scene's skinned models have more than {MAX_SCENE_JOINTS} joints together"
                ),
            ));
        }
        let root = parent * placement(&params.placement).matrix();
        // Each skin's joint matrices start, in the pose, at the first joint of the primitives it
        // moves, and later skins' start later.
        let first_skin = self.skins.len();
        self.skins
            .extend(pose.skins.iter().map(|&(skin, start)| PosedSkin {
                first_joint: first_joint + start as usize,
                root,
                joint_bounds: model.joint_bounds.clone(),
                skin,
            }));
        let skin_of = |start: u32| {
            pose.skins
                .binary_search_by_key(&start, |&(_, first)| first)
                .ok()
                .map(|index| first_skin + index)
        };
        self.scene.joints.extend(pose.joints.iter().copied());
        let default = Material::default();
        for posed in pose.primitives {
            let primitive = posed.primitive;
            let material = primitive
                .material
                .and_then(|index| scene.materials().get(index))
                .unwrap_or(&default);
            let mut surface =
                model_surface(material, &model.textures, primitive.flat_normals, &params);
            if let Some((replaced, raster)) = replaced
                && primitive.material == Some(replaced)
            {
                surface = Surface {
                    texture: Some(raster),
                    unlit: true,
                    blending: material.blending,
                    double_sided: material.double_sided,
                    cast_shadows: params.cast_shadows,
                    receive_shadows: params.receive_shadows,
                    ..Surface::default()
                };
            }
            let local = model
                .bounds
                .get(&(primitive.first_index, primitive.index_count))
                .copied()
                .unwrap_or(Bounds::EMPTY);
            // A skinned primitive's joints place it within the model, so only the model is placed.
            let world = match posed.first_joint {
                Some(_) => root,
                None => root * posed.world,
            };
            let extent = match posed.first_joint.and_then(skin_of) {
                Some(skin) => Extent::Skin(skin),
                None => Extent::Local(local),
            };
            self.push(
                Object {
                    world,
                    mesh: model.mesh,
                    first_index: primitive.first_index,
                    index_count: primitive.index_count,
                    surface,
                    center: world.transform_point3(primitive.center),
                    first_joint: posed.first_joint.map(|joint| joint + first_joint as u32),
                    bounds: Bounds::EMPTY,
                },
                extent,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use lumen_3d::model::{Model, ModelError};

    use super::{evictions, model_failure};
    use crate::error::MediaReason;
    use crate::media::AssetKind;

    #[test]
    fn evictions_free_the_least_recently_drawn_down_to_the_budget_and_spare_protected_models() {
        // Each model's key, bytes, last tick drawn and whether it must stay.
        let models = [
            ("old", 10, 1, false),
            ("kept", 10, 1, true),
            ("older", 10, 0, false),
            ("recent", 10, 2, false),
            ("drawn", 10, 3, true),
        ];
        assert_eq!(
            evictions(models.into_iter(), 50, 25,),
            ["older", "old", "recent"]
        );
        assert_eq!(evictions(models.into_iter(), 50, 35), ["older", "old"]);
        assert_eq!(evictions(models.into_iter(), 50, 50), Vec::<&str>::new());
        // Protected models stay even when what is left is over the budget.
        assert_eq!(
            evictions(models.into_iter(), 50, 0),
            ["older", "old", "recent"]
        );
    }

    /// What reading `bytes` as a binary glTF fails with: its code and the kind found.
    fn read_as_model(bytes: &[u8]) -> (&'static str, Option<AssetKind>) {
        let error = Model::from_glb(bytes).expect_err("not a readable model");
        let failure = model_failure("scene.glb", bytes, error);
        let failure = failure.failure().expect("a media failure");
        assert_eq!(failure.wanted, [AssetKind::Model]);
        assert_eq!(failure.source, "scene.glb");
        (failure.code(), failure.found)
    }

    #[test]
    fn a_model_that_cant_be_read_fails_by_what_the_file_is() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0; 32]);
        assert_eq!(
            read_as_model(&png),
            ("media.wrong_kind", Some(AssetKind::Image))
        );
        assert_eq!(
            read_as_model(br#"  {"asset": {"version": "2.0"}}"#),
            ("media.unsupported", None)
        );
        assert_eq!(
            read_as_model(b"nothing like a model at all"),
            ("media.decode", None)
        );
        assert_eq!(
            read_as_model(b"glTF\x02\0\0\0\xff\xff\xff\x7f"),
            ("media.decode", None)
        );
    }

    #[test]
    fn a_model_over_a_limit_reports_the_limit_and_the_size() {
        let error = model_failure(
            "scene.glb",
            b"glTF",
            ModelError::TooLarge {
                details: "the file is larger than the 512 MiB limit".to_string(),
                limit: 512 << 20,
                actual: Some(600 << 20),
            },
        );
        let failure = error.failure().unwrap();
        assert_eq!(failure.code(), "media.too_large");
        assert!(matches!(
            failure.reason,
            MediaReason::TooLarge { limit, actual: Some(actual), .. }
                if limit == 512 << 20 && actual == 600 << 20
        ));
    }
}
