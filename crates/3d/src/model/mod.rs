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
//! their headers. A decoder that panics fails the model where panics unwind; on targets that
//! abort on panic, such as `wasm32-unknown-unknown`, it aborts instead.
//!
//! A model holds plain data: its geometry and decoded textures to upload, and the [`ModelScene`]
//! that poses and shades them.

mod animation;
mod buffers;
mod glb;
mod image;
mod read;
mod scan;
#[cfg(test)]
mod tests;

use std::fmt;

use glam::{Mat4, Quat, Vec3};

use crate::{Blending, MAX_SCENE_NODES, Maps, SkinVertex, Vertex};
use animation::Animation;
pub use animation::Playback;

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
    TooLarge(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(details) | Self::Unsupported(details) | Self::TooLarge(details) => {
                f.write_str(details)
            }
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

pub(crate) fn too_large(what: &str, limit: impl fmt::Display) -> ModelError {
    ModelError::TooLarge(format!("the model has more {what} than the {limit} it may"))
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
                let names: Vec<String> = self
                    .materials
                    .iter()
                    .map(|material| format!("`{}`", material.name))
                    .collect();
                if names.is_empty() {
                    format!("the model has no material `{name}`; it has no materials")
                } else {
                    format!(
                        "the model has no material `{name}`; it has {}",
                        names.join(", ")
                    )
                }
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
            let clips: Vec<String> = self
                .animations
                .iter()
                .enumerate()
                .map(|(index, animation)| {
                    if animation.name.is_empty() {
                        format!("{index} (unnamed)")
                    } else {
                        format!("{index} `{}`", animation.name)
                    }
                })
                .collect();
            if clips.is_empty() {
                format!("the model has no animation `{selector}`; it has no animations")
            } else {
                format!(
                    "the model has no animation `{selector}`; it has {}",
                    clips.join(", ")
                )
            }
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
                let skin = node.skin?;
                let start = skin_starts.get_mut(skin)?;
                if start.is_none() {
                    let skin = self.skins.get(skin)?;
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
