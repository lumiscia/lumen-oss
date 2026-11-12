//! Evaluates a scene's nodes at a frame into what `render_3d` draws.

mod instance;

use std::sync::Arc;

use foldhash::HashMap;
use lumen_3d::glam::{Mat4, Vec3};
use lumen_3d::model::{
    Material, Model, ModelError, ModelScene, Playback, Reach, SkinBounds, Texture,
};
use lumen_3d::{
    Blending, Bounds, Camera, Light, LightKind, MAX_SCENE_NODES, Mesh, Object, Primitive,
    Projection, Scene, Shadow, Surface, Transform,
};
use lumen_gpu::{BufRef, Recorder, TexRef, wgpu};

use super::{Light3dKind, Material3d, Model3d, Placement3d, Pose3d, Projection3d, Shape3dKind};
use crate::error::{LumenError, MediaError, MediaFailure, RenderError};
use crate::gpu::frame_graph::{FrameCx, Raster};
use crate::media::{AssetKind, MediaIdentity, ModelResolver, resolve_as};
use crate::node::vector::paint::Paint;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeKind, required_input};

/// The most joint matrices one frame's scene may hold.
const MAX_SCENE_JOINTS: usize = 1 << 16;
/// The most morph target weights one frame's scene may hold.
const MAX_SCENE_WEIGHTS: usize = 1 << 20;
/// The shortest a directional light's shadows reach from the camera.
const MIN_SHADOW_DISTANCE: f32 = 0.01;

/// A mesh's vertices and indices on the GPU, which the scene shader pulls vertices from, and what
/// else a model's vertices hold, in 32-bit words: a skinned one's joints and weights from the
/// start, then its vertex extras from `extras`, then its morph targets' displacements from
/// `morphs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GpuMesh {
    pub(crate) vertices: BufRef,
    pub(crate) indices: BufRef,
    pub(crate) data: Option<BufRef>,
    pub(crate) extras: Option<u32>,
    pub(crate) morphs: u32,
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
            data: None,
            extras: None,
            morphs: 0,
        }
    }

    pub(super) fn forget(&self, rec: &mut Recorder) {
        for buffer in [self.vertices, self.indices].into_iter().chain(self.data) {
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
    /// The reach of each primitive's vertices, by its first index and index count.
    bounds: Arc<HashMap<(u32, u32), Reach>>,
    /// Each skin's bounds before posing.
    joint_bounds: Arc<Vec<SkinBounds>>,
}

struct CachedModel {
    model: LoadedModel,
    bytes: u64,
    last_used: u64,
}

/// What scenes draw, uploaded once: the primitives' and the image plane's meshes with how many
/// indices each draws, and models by their identity.
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

    /// The bytes of models, environment maps and text meshes held.
    pub(crate) fn held_bytes(&self) -> (u64, u64, u64) {
        (
            self.model_bytes,
            self.environments.bytes(),
            self.texts.bytes(),
        )
    }

    /// Frees environment maps as [`Self::trim`] frees models.
    pub(crate) fn trim_environments<'k>(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        kept: impl IntoIterator<Item = &'k String>,
        budget: u64,
    ) {
        self.environments
            .trim(rec, tick, &kept.into_iter().collect(), budget);
    }

    /// Frees models, least recently drawn first, while the cache holds more than the models'
    /// budget, keeping those frame `tick` drew and those `kept` names, and frees text meshes the
    /// same way.
    pub(crate) fn trim<'k>(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        kept: impl IntoIterator<Item = &'k String>,
        budgets: &crate::gpu::CacheBudgets,
    ) {
        let kept: std::collections::HashSet<&String> = kept.into_iter().collect();
        self.texts.trim(rec, tick, &kept, budgets.text_meshes);
        if self.model_bytes <= budgets.models {
            return;
        }
        let freed = evictions(
            self.models.iter().map(|(id, cached)| {
                let protected = cached.last_used == tick || kept.contains(id);
                (id, cached.bytes, cached.last_used, protected)
            }),
            self.model_bytes,
            budgets.models,
        );
        let freed: Vec<String> = freed.into_iter().cloned().collect();
        for id in freed {
            if let Some(cached) = self.models.remove(&id) {
                forget_model(rec, &cached.model);
                self.model_bytes -= cached.bytes;
            }
        }
    }

    /// The model `resolver` serves as `identity`, loaded once per identity and marked drawn in
    /// `tick`.
    fn model(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        source: &str,
        (identity, resolver): (&MediaIdentity, &dyn ModelResolver),
    ) -> Result<LoadedModel, MediaError> {
        if let Some(cached) = self.models.get_mut(identity.as_str()) {
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
            identity.as_str().to_string(),
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

/// The part of a raster that an image plane shows, in whole pixels: its left, top, right and bottom
/// edges within the raster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlanePart([u32; 4]);

/// The part of a raster `display` in size that `region`, `[x, y, width, height]`, covers, cut off
/// at the raster's edges as `crop` cuts its region; `None` if it covers no pixel.
fn plane_part(display: lumen_gpu::Size, region: [i64; 4]) -> Option<PlanePart> {
    let [x, y, width, height] = region;
    let (across, down) = (i64::from(display.width), i64::from(display.height));
    let left = x.clamp(0, across);
    let top = y.clamp(0, down);
    let right = x.saturating_add(width.max(0)).clamp(left, across);
    let bottom = y.saturating_add(height.max(0)).clamp(top, down);
    (left < right && top < bottom)
        .then(|| PlanePart([left, top, right, bottom].map(|edge| edge as u32)))
}

impl PlanePart {
    /// The edges as fractions of the raster's width and height from its top left, as
    /// [`Object::part`] takes them, or `None` for the whole raster.
    fn fractions(&self, display: lumen_gpu::Size) -> Option<[f32; 4]> {
        let [left, top, right, bottom] = self.0;
        if self.0 == [0, 0, display.width, display.height] {
            return None;
        }
        let (across, down) = (display.width as f32, display.height as f32);
        Some([
            left as f32 / across,
            top as f32 / down,
            right as f32 / across,
            bottom as f32 / down,
        ])
    }
}

/// Why `bytes`, the file `source`, is not a model that can be read.
/// Reads `bytes` as the model `source` names, and fails as a render that draws it would.
pub fn check_model(source: &str, bytes: &[u8]) -> Result<(), MediaError> {
    Model::from_glb(bytes)
        .map(drop)
        .map_err(|error| model_failure(source, bytes, error))
}

fn model_failure(source: &str, bytes: &[u8], error: ModelError) -> MediaError {
    let wanted = [AssetKind::Model];
    let failure = match error {
        ModelError::Invalid(_) if !bytes.starts_with(b"glTF") => {
            match crate::media::sniff(bytes).other_than(AssetKind::Model) {
                Some(found) => MediaFailure::wrong_kind(
                    source,
                    &wanted,
                    found,
                    "the file is not a binary glTF (.glb)",
                ),
                None if lumen_3d::model::is_gltf_json(bytes) => MediaFailure::unsupported(
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
    let data = [
        bytemuck::cast_slice::<_, u8>(&geometry.skin),
        bytemuck::cast_slice(&geometry.extras),
        bytemuck::cast_slice(&geometry.morphs),
    ]
    .concat();
    let words = |bytes: usize| (bytes / 4) as u32;
    let extras =
        (!geometry.extras.is_empty()).then(|| words(size_of_val(geometry.skin.as_slice())));
    let morphs =
        words(size_of_val(geometry.skin.as_slice()) + size_of_val(geometry.extras.as_slice()));
    let parts: [&[u8]; 3] = [
        bytemuck::cast_slice(&geometry.vertices),
        bytemuck::cast_slice(&geometry.indices),
        &data,
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
    let bounds = model.scene.primitive_bounds(&model.geometry);
    let joint_bounds = model.scene.joint_bounds(&model.geometry);
    let mut bytes = parts.iter().map(|part| part.len() as u64).sum::<u64>()
        + (bounds.len() * size_of::<((u32, u32), Reach)>()) as u64
        + joint_bounds
            .iter()
            .map(|skin| ((skin.joints.len() + 1) * size_of::<Reach>()) as u64)
            .sum::<u64>();
    let mesh = GpuMesh {
        vertices: storage_buffer(rec, "lumen 3d model vertices", parts[0]),
        indices: storage_buffer(rec, "lumen 3d model indices", parts[1]),
        data: (!data.is_empty()).then(|| storage_buffer(rec, "lumen 3d model vertex data", &data)),
        extras,
        morphs,
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
        bounds: Arc::new(bounds.into_iter().collect()),
        joint_bounds: Arc::new(joint_bounds),
    };
    Ok((loaded, bytes))
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

/// The frames, both ends included, around `frame` on which a model plays a clip that spans `span`
/// seconds as `params` say, at `fps`, and shows the same pose. A looping clip never does, and a
/// clip without a span, or a composition without a frame rate, always does.
fn held_frames(
    span: Option<(f32, f32)>,
    params: &super::Model3dParams,
    fps: f64,
    frame: f64,
) -> (f64, f64) {
    let everywhere = (f64::NEG_INFINITY, f64::INFINITY);
    let Some((start, end)) = span else {
        return everywhere;
    };
    if fps <= 0.0 {
        return everywhere;
    }
    if params.animation_loop || params.animation_speed == 0.0 {
        return (f64::INFINITY, f64::NEG_INFINITY);
    }
    // Where the clip's time is `time`, on the frame axis.
    let at = |time: f32| (f64::from(time) - params.animation_offset) / params.animation_speed * fps;
    // The pose holds from the frame the clip's end reaches, or its start when it plays backwards,
    // onwards, and up to the frame the other bound reaches.
    let (hold_from, hold_to) = if params.animation_speed > 0.0 {
        (at(end), at(start))
    } else {
        (at(start), at(end))
    };
    // A frame is held once its time is clear of the bounds' rounding to `f32`.
    let margin = |bound: f64| 1e-3 + 1e-6 * bound.abs();
    if frame >= hold_from + margin(hold_from) {
        (hold_from + margin(hold_from), f64::INFINITY)
    } else if frame <= hold_to - margin(hold_to) {
        (f64::NEG_INFINITY, hold_to - margin(hold_to))
    } else {
        (f64::INFINITY, f64::NEG_INFINITY)
    }
}

/// How a model's `material` shades `primitive`, reading the model's `textures`.
fn model_surface(
    material: &Material,
    textures: &[TexRef],
    primitive: &lumen_3d::model::Primitive,
    params: &super::Model3dParams,
) -> Surface<TexRef> {
    Surface {
        base_color: material.base_color,
        texture: None,
        // Reading checked that every map names one of the model's textures.
        maps: material.maps.map(|&texture| textures[texture]),
        map_uvs: material.map_uvs,
        vertex_colors: primitive.vertex_colors,
        metallic: material.metallic,
        roughness: material.roughness,
        emissive: material.emissive,
        normal_scale: material.normal_scale,
        occlusion_strength: material.occlusion_strength,
        blending: material.blending,
        unlit: material.unlit,
        double_sided: material.double_sided,
        flat_normals: primitive.flat_normals,
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
/// matrix they are placed by, its joints' bounds before posing, and the largest size of the
/// weights morphing the primitives it moves.
struct PosedSkin {
    first_joint: usize,
    root: Mat4,
    joint_bounds: Arc<Vec<SkinBounds>>,
    skin: usize,
    weight: f32,
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
    /// The rasters objects show, each with the node it is the output of.
    pub(super) rasters: Vec<(NodeId, Raster)>,
    /// The mesh of image planes, once the scene has one.
    pub(super) quad: Option<GpuMesh>,
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
            rasters: Vec::new(),
            quad: None,
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
                    .filter(|(reach, _)| !reach.still.is_empty())
                    .fold(
                        bounds.unmoved.at(skin.weight).transformed(&skin.root),
                        |all, (reach, joint)| {
                            all.union(reach.at(skin.weight).transformed(&(skin.root * *joint)))
                        },
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
                        morph: None,
                        bounds: Bounds::EMPTY,
                        part: None,
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
                            morph: None,
                            bounds: Bounds::EMPTY,
                            part: None,
                        },
                        Extent::Local(local),
                    );
                }
            }
            NodeKind::ImagePlane3d(plane) => {
                let params = plane.params.eval(&ctx)?;
                let source_node = required_input(node_id, "source", plane.source)?;
                let source = cx.input_native(source_node)?;
                let display = source.domain.display_size();
                let part = match params.region.0 {
                    Some(region) => match plane_part(display, region) {
                        Some(part) => part.fractions(display),
                        None => return Ok(None),
                    },
                    None => None,
                };
                self.rasters.push((source_node, source));
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
                self.quad = Some(mesh);
                let [left, top, right, bottom] = part.unwrap_or([0.0, 0.0, 1.0, 1.0]);
                let local = Bounds {
                    min: Vec3::new(left - 0.5, 0.5 - bottom, 0.0),
                    max: Vec3::new(right - 0.5, 0.5 - top, 0.0),
                };
                self.push(
                    Object {
                        world,
                        mesh,
                        first_index: 0,
                        index_count,
                        surface,
                        center: world.transform_point3((local.min + local.max) * 0.5),
                        first_joint: None,
                        morph: None,
                        bounds: Bounds::EMPTY,
                        part,
                    },
                    Extent::Local(local),
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
        let (identity, resolver) =
            resolve_as::<dyn ModelResolver>(media, source).map_err(in_node)?;
        let tick = cx.tick();
        let (rec, cache) = cx.scene_cache();
        let model = cache
            .model(rec, tick, source, (&identity, &*resolver))
            .map_err(in_node)?;
        cx.drew_model(identity.as_str());
        cx.read_source(AssetKind::Model, source, Some(&identity));
        let scene = &model.scene;
        if scene.skipped_primitives() > 0 {
            cx.note_skipped_primitives(node.id, source, scene.skipped_primitives());
        }
        let clip = scene.animation(&params.animation).map_err(error)?;
        let camera = scene.camera(&params.camera).map_err(error)?;
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
                    Some(texture) => {
                        let raster = cx.input_native(texture)?;
                        self.rasters.push((texture, raster));
                        Some((material, raster.tex))
                    }
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
        if let Some(clip) = clip
            && node.animates()
        {
            cx.held_on(held_frames(
                scene.clip_span(clip),
                &params,
                f64::from(cx.expr().fps),
                cx.frame_time().value(),
            ));
        }
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
        let first_weight = self.scene.weights.len();
        if first_weight + pose.weights.len() > MAX_SCENE_WEIGHTS {
            return Err(super::render::render_error(
                self.render_id,
                format!(
                    "the scene's morphed models have more than {MAX_SCENE_WEIGHTS} morph target \
                     weights together"
                ),
            ));
        }
        self.scene.weights.extend_from_slice(&pose.weights);
        let root = parent * placement(&params.placement).matrix();
        if params.lights {
            self.scene
                .lights
                .extend(pose.lights.iter().map(|light| light.placed(&root)));
            self.light_nodes
                .extend(std::iter::repeat_n(node.id, pose.lights.len()));
        }
        if let Some(camera) = camera
            && let Some(&(_, world)) = pose.cameras.iter().find(|(index, _)| *index == camera)
            && let Some(model_camera) = scene.cameras().get(camera)
        {
            // A camera keeps its node's place and turn, but not its scale.
            let (_, rotation, translation) = (root * world).to_scale_rotation_translation();
            let world = Mat4::from_rotation_translation(rotation, translation);
            if world.is_finite() {
                self.scene.camera.get_or_insert(Camera {
                    world,
                    projection: model_camera.projection,
                    near: model_camera.near,
                    far: model_camera.far,
                });
            }
        }
        // Each skin's joint matrices start, in the pose, at the first joint of the primitives it
        // moves, and later skins' start later.
        let first_skin = self.skins.len();
        self.skins
            .extend(pose.skins.iter().map(|&(skin, start)| PosedSkin {
                first_joint: first_joint + start as usize,
                root,
                joint_bounds: model.joint_bounds.clone(),
                skin,
                weight: 0.0,
            }));
        let skin_of = |start: u32| {
            pose.skins
                .binary_search_by_key(&start, |&(_, first)| first)
                .ok()
                .map(|index| first_skin + index)
        };
        self.scene.joints.extend(pose.joints.iter().copied());
        let default = Material::default();
        for posed in &pose.primitives {
            let primitive = posed.primitive;
            let weight = pose.weight_scale(posed);
            let material = primitive
                .material
                .and_then(|index| scene.materials().get(index))
                .unwrap_or(&default);
            let mut surface = model_surface(material, &model.textures, &primitive, &params);
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
                .map_or(Bounds::EMPTY, |reach| reach.at(weight));
            // A skinned primitive's joints place it within the model, so only the model is placed.
            let world = match posed.first_joint {
                Some(_) => root,
                None => root * posed.world,
            };
            let extent = match posed.first_joint.and_then(skin_of) {
                Some(skin) => {
                    let posed_skin = &mut self.skins[skin];
                    posed_skin.weight = posed_skin.weight.max(weight);
                    Extent::Skin(skin)
                }
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
                    morph: primitive
                        .morph
                        .zip(posed.first_weight)
                        .map(|(morph, weight)| (morph, weight + first_weight as u32)),
                    bounds: Bounds::EMPTY,
                    part: None,
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

    use lumen_gpu::Size;

    use super::{PlanePart, evictions, held_frames, model_failure, plane_part};
    use crate::error::MediaReason;
    use crate::media::AssetKind;
    use crate::node::scene::Model3dParams;

    fn clip(speed: f64, offset: f64, looped: bool) -> Model3dParams {
        Model3dParams {
            animation_speed: speed,
            animation_offset: offset,
            animation_loop: looped,
            ..Model3dParams::default()
        }
    }

    #[test]
    fn a_region_is_cut_off_at_the_raster_as_a_crop_is() {
        let size = Size::new(100, 50);
        let part = |region| plane_part(size, region);
        assert_eq!(part([10, 5, 20, 10]), Some(PlanePart([10, 5, 30, 15])));
        assert_eq!(part([-10, -5, 30, 10]), Some(PlanePart([0, 0, 20, 5])));
        assert_eq!(part([90, 40, 100, 100]), Some(PlanePart([90, 40, 100, 50])));
        assert_eq!(
            part([0, 0, i64::MAX, i64::MAX]),
            Some(PlanePart([0, 0, 100, 50]))
        );
        assert_eq!(part([100, 0, 10, 10]), None);
        assert_eq!(part([-30, 0, 20, 10]), None);
        assert_eq!(part([10, 10, 0, 10]), None);
        assert_eq!(part([10, 10, -5, 10]), None);
    }

    #[test]
    fn a_region_covers_the_fractions_of_the_raster_it_names() {
        let size = Size::new(100, 50);
        let part = plane_part(size, [25, 10, 50, 20]).expect("a region");
        assert_eq!(part.fractions(size), Some([0.25, 0.2, 0.75, 0.6]));
        let whole = plane_part(size, [-5, 0, 200, 50]).expect("a region");
        assert_eq!(whole.fractions(size), None);
    }

    #[test]
    fn a_pose_is_held_on_the_frames_the_clip_is_clear_of_its_keys() {
        let span = Some((0.5, 2.0));
        let held = |params: &Model3dParams, frame: f64| held_frames(span, params, 10.0, frame);
        let contains = |(from, to): (f64, f64), frame: f64| from <= frame && frame <= to;
        // Playing forwards, the clip is over from frame 20 and not begun up to frame 5.
        let forwards = clip(1.0, 0.0, false);
        assert!(contains(held(&forwards, 30.0), 21.0));
        assert!(!contains(held(&forwards, 30.0), 19.0));
        assert!(contains(held(&forwards, 3.0), 4.0));
        assert!(!contains(held(&forwards, 3.0), 6.0));
        assert!(
            !contains(held(&forwards, 12.0), 12.0),
            "a playing clip holds nothing"
        );
        // Twice as fast, with an offset of a second: over from frame 5, not begun up to frame -2.5.
        let quick = clip(2.0, 1.0, false);
        assert!(contains(held(&quick, 8.0), 5.5));
        assert!(!contains(held(&quick, 8.0), 4.5));
        assert!(contains(held(&quick, -5.0), -3.0));
        assert!(!contains(held(&quick, -5.0), -2.0));
        // Backwards, the clip is at its first key from frame -5 on and at its last up to frame -20.
        let backwards = clip(-1.0, 0.0, false);
        assert!(contains(held(&backwards, 0.0), 1.0));
        assert!(!contains(held(&backwards, 0.0), -6.0));
        assert!(contains(held(&backwards, -30.0), -21.0));
        assert!(!contains(held(&backwards, -10.0), -10.0));
        // A looped clip is never held, and one without keys, or frames, always is.
        assert!(!contains(held(&clip(1.0, 0.0, true), 30.0), 30.0));
        let everywhere = held_frames(None, &forwards, 10.0, 7.0);
        assert!(contains(everywhere, -1e9) && contains(everywhere, 1e9));
        let timeless = held_frames(span, &forwards, 0.0, 7.0);
        assert!(contains(timeless, 7.0));
    }

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
        // A file that only starts like JSON is not taken for a glTF document.
        for json in [
            &b"{garbage"[..],
            br#"{"name": "a", "version": "1.0"}"#,
            b"{",
        ] {
            assert_eq!(read_as_model(json), ("media.decode", None));
        }
        // A video or audio container is not a model, whichever it holds.
        assert_eq!(
            read_as_model(b"\0\0\0\x18ftypmp42\0\0\0\0"),
            ("media.wrong_kind", None)
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
