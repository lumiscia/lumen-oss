//! Binary glTF (`.glb`) models, read from untrusted bytes by [`Model::from_glb`].
//!
//! Reading is safe and bounded on any input: it checks every length, index and count against the
//! limits below before it uses or allocates for it, does its arithmetic without overflow, and
//! fails with a [`ModelError`] rather than panicking. What it reads from a valid file is correct,
//! and every number it keeps is finite. It is not a glTF validator: it accepts some files the
//! specification forbids where they can still be read safely, such as bytes past the declared
//! length, nodes no scene reaches that form a cycle, repeated key times, and two animation
//! channels that target the same property, of which the later wins.
//!
//! Images are decoded by the `image` crate, after their size is checked against the limits from
//! their headers. With the crate's features, compressed data is read too: Draco primitives
//! (`draco`), meshopt buffer views (`meshopt`), KTX2 Basis Universal images (`ktx2`) and WebP
//! images (`webp`), each checked against the limits from what the JSON and its headers declare
//! before it is decoded, and its decoded size against what they declared after. A decoder that
//! panics fails the model where panics unwind; on targets that abort on panic, such as
//! `wasm32-unknown-unknown`, it aborts instead.
//!
//! A model holds plain data: its geometry and decoded textures to upload, and the [`ModelScene`]
//! that poses and shades them.

mod animation;
mod bounds;
mod buffers;
#[cfg(feature = "draco")]
mod draco;
mod glb;
mod gltf_json;
mod image;
mod info;
#[cfg(feature = "ktx2")]
mod ktx2;
#[cfg(feature = "meshopt")]
mod meshopt;
mod read;
mod scan;
#[cfg(test)]
mod tests;

use std::fmt;

use glam::{Mat4, Quat, Vec3};

use crate::{Blending, MAX_SCENE_NODES, Maps, SkinVertex, Vertex};
use animation::Animation;
pub use animation::Playback;
pub use bounds::SkinBounds;
pub use gltf_json::{GLTF_JSON_SNIFF_BYTES, is_gltf_json};
pub use info::{AnimationInfo, MaterialInfo, ModelInfo};

/// The largest file read.
pub const MAX_MODEL_BYTES: usize = 512 << 20;
/// The largest JSON chunk read.
pub const MAX_JSON_BYTES: usize = 32 << 20;
/// The most values (objects, arrays, keys, strings, numbers, booleans and nulls) the JSON chunk
/// may hold together, which bounds what deserialising it allocates.
pub const MAX_JSON_VALUES: usize = 4_000_000;
pub use scan::{COLLECTION_LIMITS, MAX_ANIMATION_CHANNELS, MAX_PRIMITIVES};
/// The most vertices all of a model's primitives may hold together.
pub const MAX_VERTICES: usize = 4_000_000;
/// The most triangle indices all of a model's primitives may hold together.
pub const MAX_INDICES: usize = 12_000_000;
/// The most nodes a model may hold.
pub const MAX_NODES: usize = 100_000;
/// The most primitives a model's scene may place, counting one each time a node places it.
pub const MAX_DRAWS: usize = MAX_SCENE_NODES;
/// The most joints one skin may hold.
pub const MAX_SKIN_JOINTS: usize = 1024;
/// The most joints all of a model's skins may hold together.
pub const MAX_JOINTS: usize = 16 * MAX_SKIN_JOINTS;
/// The most bytes one accessor may hold.
pub const MAX_VIEW_BYTES: usize = 256 << 20;
/// The most components (scalars) one accessor may hold.
pub const MAX_COMPONENTS: usize = 48_000_000;
/// The most bytes all accessor reads may decode to together, at four bytes a component.
pub const MAX_DECODED_BYTES: usize = 512 << 20;
/// Accessor reads may also decode to no more than this many times the file's size, or
/// [`MIN_DECODED_BUDGET`] for a smaller file, so that a small file can't expand hugely.
pub const DECODED_BYTES_PER_FILE_BYTE: usize = 32;
pub const MIN_DECODED_BUDGET: usize = 16 << 20;
/// Compressed geometry, whose encoding doesn't bound how far it expands, is charged before it is
/// decoded to a budget of this many times the file's size, or [`MIN_DECODED_BUDGET`] for a
/// smaller file: one for meshopt buffer views, at their decoded size and [`MAX_DECODED_BYTES`] at
/// most, and one for Draco primitives, at what their decoder may produce and
/// [`MAX_DRACO_DECODED_BYTES`] at most. What accessors then read from meshopt views counts toward
/// the accessors' own budget as well.
pub const COMPRESSED_BYTES_PER_FILE_BYTE: usize = 256;
/// The most bytes all of a model's Draco primitives may decode to, whose decoder peaks at about
/// six times what it produces.
pub const MAX_DRACO_DECODED_BYTES: usize = 96 << 20;
/// A model's KTX2 and WebP images, whose encoding doesn't bound how many pixels they expand to,
/// may hold no more pixels together, counting every mip level, than this many for each of their
/// bytes, or [`MIN_COMPRESSED_IMAGE_PIXELS`] for smaller images.
pub const COMPRESSED_IMAGE_PIXELS_PER_BYTE: u64 = 1024;
/// Room for one 2048 x 2048 texture and its mips, however small its file.
pub const MIN_COMPRESSED_IMAGE_PIXELS: u64 = 1 << 23;
/// The widest or tallest image a model may hold, in pixels.
pub const MAX_IMAGE_SIDE: u32 = 8192;
/// The most pixels all of a model's textures may hold together, counting every mip level.
pub const MAX_TEXTURE_PIXELS: u64 = 160_000_000;
/// The most bytes decoding one image may produce, in its own pixel format: an 8-bit RGBA image
/// [`MAX_IMAGE_SIDE`] a side.
pub const MAX_IMAGE_DECODE_BYTES: u64 = 1 << 28;

/// Why a model can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// Not a well-formed binary glTF, or one whose data contradicts itself.
    Invalid(String),
    /// A well-formed model that needs something this reader doesn't do.
    Unsupported(String),
    /// A model holding more of something than the limits allow.
    TooLarge {
        details: String,
        limit: u64,
        /// The size over the limit, when reading found it; it stops at the limit otherwise.
        actual: Option<u64>,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(details)
            | Self::Unsupported(details)
            | Self::TooLarge { details, .. } => f.write_str(details),
        }
    }
}

impl std::error::Error for ModelError {}

pub(crate) fn invalid(details: impl Into<String>) -> ModelError {
    ModelError::Invalid(details.into())
}

pub(crate) fn unsupported(details: impl Into<String>) -> ModelError {
    ModelError::Unsupported(details.into())
}

pub(crate) fn too_large<L>(what: &str, limit: L) -> ModelError
where
    L: fmt::Display + TryInto<u64> + Copy,
{
    ModelError::TooLarge {
        details: format!("the model has more {what} than the {limit} it may"),
        limit: limit.try_into().unwrap_or(u64::MAX),
        actual: None,
    }
}

impl ModelError {
    /// A `TooLarge` that says how large the model was.
    pub(crate) fn of_size(self, size: usize) -> Self {
        match self {
            Self::TooLarge { details, limit, .. } => Self::TooLarge {
                details,
                limit,
                actual: Some(size as u64),
            },
            error => error,
        }
    }
}

/// Runs a third-party decoder, failing with `what` is corrupt rather than taking the process down
/// if it panics, where panics unwind. Where they abort, as on `wasm32-unknown-unknown`, a panic
/// aborts.
pub(crate) fn guarded<T>(
    what: &str,
    decode: impl FnOnce() -> Result<T, ModelError>,
) -> Result<T, ModelError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(decode))
        .unwrap_or_else(|_| Err(invalid(format!("{what} in the model is corrupt"))))
}

/// A model read from a `.glb`: what to upload once, and what poses and shades it each frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub geometry: Geometry,
    /// The images materials read, decoded, each in the colour space its material slots read it in.
    pub textures: Vec<Texture>,
    pub scene: ModelScene,
}

impl Model {
    /// Reads and checks a binary glTF whose buffers and images are all inside it, either in its
    /// binary chunk or as base64 `data:` URIs.
    pub fn from_glb(bytes: &[u8]) -> Result<Self, ModelError> {
        read::read(bytes)
    }
}

/// Every primitive's vertices and indices, one primitive after another.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Geometry {
    pub vertices: Vec<Vertex>,
    /// Indices into all of `vertices`, three to a triangle.
    pub indices: Vec<u32>,
    /// One per vertex when any primitive is skinned, and empty otherwise.
    pub skin: Vec<SkinVertex>,
}

/// A decoded image with its mip levels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    /// Whether texels hold sRGB-encoded colour, as base colour and emissive textures do, rather
    /// than linear values.
    pub srgb: bool,
    /// RGBA8 rows, top first, of each level from the full size down to 1 x 1.
    pub levels: Vec<Vec<u8>>,
}

/// A metallic-roughness material, in linear light. Maps name textures by their index in
/// [`Model::textures`].
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub name: String,
    /// Straight alpha.
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// Including `KHR_materials_emissive_strength`.
    pub emissive: [f32; 3],
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub blending: Blending,
    /// `KHR_materials_unlit`.
    pub unlit: bool,
    pub double_sided: bool,
    pub maps: Maps<usize>,
}

impl Default for Material {
    /// glTF's material for primitives that name none.
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            blending: Blending::Opaque,
            unlit: false,
            double_sided: false,
            maps: Maps::default(),
        }
    }
}

/// One triangle primitive's run of [`Geometry::indices`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Primitive {
    pub first_index: u32,
    pub index_count: u32,
    /// Its index in [`ModelScene::materials`], or `None` for [`Material::default`].
    pub material: Option<usize>,
    /// The centre of its bounds, in the space of the node that places it.
    pub center: Vec3,
    /// Has no vertex normals, so each face is lit by its own.
    pub flat_normals: bool,
    /// Has joints and weights, so a node with a skin moves it by its joints.
    pub skinned: bool,
}

/// A node of the hierarchy, in its parent's space: `matrix` as the file gives it when no animation
/// moves the node, and otherwise its translation, rotation and scale.
#[derive(Debug, Clone, PartialEq)]
struct Node {
    matrix: Option<Mat4>,
    translation: Vec3,
    rotation: Quat,
    scale: Vec3,
    children: Vec<usize>,
    mesh: Option<usize>,
    skin: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
struct Skin {
    joints: Vec<usize>,
    /// One per joint.
    inverse_binds: Vec<Mat4>,
}

/// Materials, the node hierarchy of the default scene, skins and animation clips.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelScene {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    /// The primitives of each mesh.
    meshes: Vec<Vec<Primitive>>,
    materials: Vec<Material>,
    skins: Vec<Skin>,
    animations: Vec<Animation>,
    /// Primitives a pose places.
    draws: usize,
}

/// A primitive placed in model space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Posed {
    pub primitive: Primitive,
    /// The world matrix of the node that places it.
    pub world: Mat4,
    /// Where its joint matrices start in [`Pose::joints`], when it is skinned.
    pub first_joint: Option<u32>,
}

/// A model's primitives placed at one moment of a clip, in model space.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pose {
    pub primitives: Vec<Posed>,
    /// The joint matrices of each skin a skinned primitive uses, which take its vertices to
    /// model space.
    pub joints: Vec<Mat4>,
    /// Each skin whose joint matrices are in `joints`, by its index, with where they start.
    pub skins: Vec<(usize, u32)>,
}

/// The most names an error lists.
const LISTED_NAMES: usize = 20;

/// `it has` the first [`LISTED_NAMES`] of `count` `names`, or says it has none of `what`.
fn listed(names: impl Iterator<Item = String>, count: usize, what: &str) -> String {
    if count == 0 {
        return format!("it has no {what}");
    }
    let mut list: Vec<String> = names.take(LISTED_NAMES).collect();
    if count > LISTED_NAMES {
        list.push(format!("and {} more", count - LISTED_NAMES));
    }
    format!("it has {}", list.join(", "))
}

impl ModelScene {
    pub fn materials(&self) -> &[Material] {
        &self.materials
    }

    /// How many primitives [`Self::pose`] places.
    pub fn draw_count(&self) -> usize {
        self.draws
    }

    /// The material named `name`, or an error listing those the model has.
    pub fn material(&self, name: &str) -> Result<usize, String> {
        self.materials
            .iter()
            .position(|material| material.name == name)
            .ok_or_else(|| {
                let names = self
                    .materials
                    .iter()
                    .map(|material| format!("`{}`", material.name));
                format!(
                    "the model has no material `{name}`; {}",
                    listed(names, self.materials.len(), "materials")
                )
            })
    }

    /// The clip `selector` names, by its name or else its index; empty names none. A clip the
    /// model lacks is an error listing those it has.
    pub fn animation(&self, selector: &str) -> Result<Option<usize>, String> {
        if selector.is_empty() {
            return Ok(None);
        }
        let by_name = self
            .animations
            .iter()
            .position(|animation| animation.name == selector);
        let by_index = || {
            selector
                .parse::<usize>()
                .ok()
                .filter(|&index| index < self.animations.len())
        };
        by_name.or_else(by_index).map(Some).ok_or_else(|| {
            let clips = self
                .animations
                .iter()
                .enumerate()
                .map(|(index, animation)| {
                    if animation.name.is_empty() {
                        format!("{index} (unnamed)")
                    } else {
                        format!("{index} `{}`", animation.name)
                    }
                });
            format!(
                "the model has no animation `{selector}`; {}",
                listed(clips, self.animations.len(), "animations")
            )
        })
    }

    /// The scene's primitives placed with `playback`'s clip sampled, or at rest without one. A
    /// primitive whose world matrix, or any of whose joint matrices, isn't finite is left out, so
    /// every matrix in the pose is finite.
    pub fn pose(&self, playback: Option<Playback>) -> Pose {
        let mut locals: Vec<(Vec3, Quat, Vec3)> = self
            .nodes
            .iter()
            .map(|node| (node.translation, node.rotation, node.scale))
            .collect();
        if let Some(playback) = playback
            && let Some(animation) = self.animations.get(playback.clip)
        {
            animation.apply(playback.time(animation.duration), &mut locals);
        }

        // Reading checked that every node has one parent at most and that roots have none, so
        // this reaches each node once.
        let mut worlds = vec![Mat4::IDENTITY; self.nodes.len()];
        let mut order = Vec::with_capacity(self.nodes.len());
        let mut pending: Vec<(usize, Mat4)> = self
            .roots
            .iter()
            .rev()
            .map(|&root| (root, Mat4::IDENTITY))
            .collect();
        while let Some((index, parent)) = pending.pop() {
            let (Some(node), Some(&(translation, rotation, scale))) =
                (self.nodes.get(index), locals.get(index))
            else {
                continue;
            };
            let local = node.matrix.unwrap_or_else(|| {
                Mat4::from_scale_rotation_translation(scale, rotation, translation)
            });
            let world = parent * local;
            worlds[index] = world;
            order.push(index);
            pending.extend(node.children.iter().rev().map(|&child| (child, world)));
        }

        let mut pose = Pose {
            primitives: Vec::with_capacity(self.draws),
            joints: Vec::new(),
            skins: Vec::new(),
        };
        // Where each skin's joint matrices start, once worked out, or `None` when any isn't finite.
        let mut skin_starts: Vec<Option<Option<u32>>> = vec![None; self.skins.len()];
        for index in order {
            let node = &self.nodes[index];
            let Some(primitives) = node.mesh.and_then(|mesh| self.meshes.get(mesh)) else {
                continue;
            };
            let world = worlds[index];
            if !world.is_finite() {
                continue;
            }
            let mut skin_start = || {
                let index = node.skin?;
                let start = skin_starts.get_mut(index)?;
                if start.is_none() {
                    let skin = self.skins.get(index)?;
                    let joints: Vec<Mat4> = skin
                        .joints
                        .iter()
                        .zip(&skin.inverse_binds)
                        .map(|(&joint, inverse_bind)| {
                            worlds.get(joint).copied().unwrap_or(Mat4::NAN) * *inverse_bind
                        })
                        .collect();
                    *start = Some(joints.iter().all(Mat4::is_finite).then(|| {
                        let first = pose.joints.len() as u32;
                        pose.joints.extend(joints);
                        pose.skins.push((index, first));
                        first
                    }));
                }
                *start
            };
            // Skinned primitives of a node without a skin are placed by the node; those of a node
            // whose skin has a joint matrix that isn't finite are left out.
            let skinned = primitives.iter().any(|primitive| primitive.skinned);
            let skin = match node.skin {
                Some(_) if skinned => match skin_start() {
                    Some(Some(start)) => Ok(Some(start)),
                    _ => Err(()),
                },
                _ => Ok(None),
            };
            pose.primitives.extend(
                primitives
                    .iter()
                    .filter(|primitive| !primitive.skinned || skin.is_ok())
                    .map(|&primitive| Posed {
                        primitive,
                        world,
                        first_joint: skin.ok().flatten().filter(|_| primitive.skinned),
                    }),
            );
        }
        pose
    }
}
