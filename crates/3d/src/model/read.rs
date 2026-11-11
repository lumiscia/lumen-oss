//! Reads a binary glTF's document into a [`Model`], checking it as it goes.

use std::borrow::Cow;
use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
#[cfg(feature = "draco")]
use gltf_json::accessor::ComponentType;
use gltf_json::animation::{Interpolation as JsonInterpolation, Property};
use gltf_json::material::AlphaMode;
use gltf_json::mesh::{Mode, Semantic};
use gltf_json::texture::{MagFilter, MinFilter, WrappingMode};
use gltf_json::validation::Checked;
use gltf_json::{Accessor, Index, Root};

use super::animation::{Animation, Channel, Interpolation, Values, unit};
use super::buffers::{Buffers, ByteRange, data_uri, data_uri_length, size};
use super::image;
use super::{
    Geometry, MAX_DRAWS, MAX_INDICES, MAX_JOINTS, MAX_SKIN_JOINTS, MAX_TEXTURE_PIXELS,
    MAX_VERTICES, Material, Model, ModelError, ModelScene, Node, Primitive, Skin, Texture, glb,
    invalid, scan, too_large, unsupported,
};
use crate::{Blending, Filter, Map, Maps, Sampler, SkinVertex, Vertex, Wrap};

/// Extensions a model may require: the others change what it shows, so it can't be shown without
/// them.
const SUPPORTED_EXTENSIONS: &[&str] = &[
    "KHR_materials_emissive_strength",
    "KHR_materials_unlit",
    "KHR_mesh_quantization",
    #[cfg(feature = "draco")]
    "KHR_draco_mesh_compression",
    #[cfg(feature = "meshopt")]
    "EXT_meshopt_compression",
    #[cfg(feature = "ktx2")]
    "KHR_texture_basisu",
    #[cfg(feature = "webp")]
    "EXT_texture_webp",
];

/// The image a texture shows: the one its `KHR_texture_basisu` or `EXT_texture_webp` extension
/// names, when this reader decodes it, and otherwise its own source.
fn texture_source(texture: &gltf_json::Texture) -> Result<usize, ModelError> {
    let extensions = texture
        .extensions
        .as_ref()
        .map(|extensions| &extensions.others);
    let names: &[&str] = &[
        #[cfg(feature = "ktx2")]
        "KHR_texture_basisu",
        #[cfg(feature = "webp")]
        "EXT_texture_webp",
    ];
    for &name in names {
        if let Some(extension) = extensions.and_then(|extensions| extensions.get(name)) {
            return extension
                .get("source")
                .and_then(serde_json::Value::as_u64)
                .and_then(|source| usize::try_from(source).ok())
                .ok_or_else(|| invalid(format!("a texture's {name} extension names no image")));
        }
    }
    Ok(texture.source.value())
}

pub(super) fn read(bytes: &[u8]) -> Result<Model, ModelError> {
    let glb = glb::split(bytes)?;
    scan::check(glb.json)?;
    let root = Root::from_slice(glb.json)
        .map_err(|error| invalid(format!("the model's JSON isn't valid glTF: {error}")))?;
    if !root.asset.version.starts_with("2.") {
        return Err(unsupported(format!(
            "the model is glTF version {}; only glTF 2.0 is read",
            root.asset.version
        )));
    }
    if let Some(required) = root
        .extensions_required
        .iter()
        .find(|name| !SUPPORTED_EXTENSIONS.contains(&name.as_str()))
    {
        return Err(unsupported(format!(
            "the model requires the glTF extension {required}, which isn't supported"
        )));
    }
    let buffers = Buffers::load(&root, glb.bin, bytes.len())?;
    let Meshes {
        geometry,
        meshes,
        joints_needed,
    } = read_meshes(&root, &buffers)?;
    let skins = read_skins(&root, &buffers)?;
    let (nodes, roots) = read_nodes(&root, meshes.len(), skins.len())?;
    let reached = reached(&nodes, &roots);
    for (node, _) in nodes.iter().zip(&reached).filter(|(_, reached)| **reached) {
        let Some(skin) = node.skin else { continue };
        if let Some(mesh) = node.mesh
            && joints_needed[mesh] > skins[skin].joints.len()
        {
            return Err(invalid("a mesh uses a joint its skin lacks"));
        }
        if let Some(joint) = skins[skin].joints.iter().find(|&&joint| !reached[joint]) {
            return Err(invalid(format!(
                "skin {skin} uses node {joint} as a joint, which isn't in the scene"
            )));
        }
    }
    let draws = count_draws(&nodes, &reached, &meshes)?;
    let (materials, textures) = read_materials(&root, &buffers, bytes.len())?;
    if meshes.iter().flatten().any(|primitive| {
        primitive
            .material
            .is_some_and(|index| index >= materials.len())
    }) {
        return Err(invalid("a primitive names a material the model lacks"));
    }
    let animations = read_animations(&root, &buffers, nodes.len())?;
    Ok(Model {
        geometry,
        textures,
        scene: ModelScene {
            nodes,
            roots,
            meshes,
            materials,
            skins,
            animations,
            draws,
        },
    })
}

struct Meshes {
    geometry: Geometry,
    meshes: Vec<Vec<Primitive>>,
    /// For each mesh, how many of its skin's joints its vertices need.
    joints_needed: Vec<usize>,
}

/// Every triangle primitive's geometry, one after another. Primitives of other modes, and those
/// without positions, are left out. Each primitive's counts are checked against the limits before
/// its attributes are read or decoded, and what is read against them after.
fn read_meshes(root: &Root, buffers: &Buffers<'_>) -> Result<Meshes, ModelError> {
    let mut geometry = Geometry::default();
    let mut meshes = Vec::with_capacity(root.meshes.len());
    let mut joints_needed = Vec::with_capacity(root.meshes.len());
    for mesh in &root.meshes {
        let mut primitives = Vec::new();
        let mut needed = 0;
        for primitive in &mesh.primitives {
            if primitive.mode != Checked::Valid(Mode::Triangles) {
                continue;
            }
            let attribute = |semantic: Semantic| {
                primitive
                    .attributes
                    .get(&Checked::Valid(semantic))
                    .map(|&index| buffers.accessor(index))
                    .transpose()
            };
            let Some(positions) = attribute(Semantic::Positions)? else {
                continue;
            };
            let count = size(positions.count.0, "an accessor's count")?;
            let start = geometry.vertices.len();
            if count > MAX_VERTICES - start {
                return Err(too_large("vertices", MAX_VERTICES));
            }
            let indices = primitive
                .indices
                .map(|indices| buffers.accessor(indices))
                .transpose()?;
            let index_count = match indices {
                Some(indices) => size(indices.count.0, "an accessor's count")?,
                None => count,
            };
            if index_count > MAX_INDICES - geometry.indices.len() {
                return Err(too_large("indices", MAX_INDICES));
            }
            if !index_count.is_multiple_of(3) {
                return Err(invalid(
                    "a primitive's index count isn't a multiple of three",
                ));
            }
            let skin = match (
                attribute(Semantic::Joints(0))?,
                attribute(Semantic::Weights(0))?,
            ) {
                (Some(joints), Some(weights)) => Some((joints, weights)),
                _ => None,
            };
            let accessors = Accessors {
                positions,
                normals: attribute(Semantic::Normals)?,
                uvs: attribute(Semantic::TexCoords(0))?,
                skin,
                indices,
            };
            let Attributes {
                positions,
                normals,
                uvs,
                skin,
                indices,
            } = read_attributes(buffers, primitive, &accessors, count, index_count)?;

            if positions.len() != count {
                return Err(different_counts());
            }
            if positions.iter().flatten().any(|value| !value.is_finite()) {
                return Err(invalid("a vertex position isn't a finite number"));
            }
            if normals
                .as_ref()
                .is_some_and(|normals| normals.len() != count)
            {
                return Err(different_counts());
            }
            if normals
                .iter()
                .flatten()
                .flatten()
                .any(|value| !value.is_finite())
            {
                return Err(invalid("a vertex normal isn't a finite number"));
            }
            if uvs.as_ref().is_some_and(|uvs| uvs.len() != count) {
                return Err(different_counts());
            }
            if uvs
                .iter()
                .flatten()
                .flatten()
                .any(|value| !value.is_finite())
            {
                return Err(invalid(
                    "a vertex's texture coordinates aren't finite numbers",
                ));
            }
            let skin = match skin {
                Some((joints, weights)) => {
                    if joints.len() != count || weights.len() != count {
                        return Err(different_counts());
                    }
                    Some(skin_vertices(&joints, &weights, &mut needed)?)
                }
                None => None,
            };
            let local = indices.unwrap_or_else(|| (0..count as u32).collect());
            if local.len() != index_count {
                return Err(invalid(
                    "a primitive has a different number of indices than its accessor declares",
                ));
            }
            if local.iter().any(|&index| index as usize >= count) {
                return Err(invalid("a primitive has an index past its vertices"));
            }

            let mut min = Vec3::splat(f32::MAX);
            let mut max = Vec3::splat(f32::MIN);
            for &position in &positions {
                min = min.min(position.into());
                max = max.max(position.into());
            }
            let uvs = uvs.unwrap_or_else(|| planar_uvs(&positions, min, max));
            let flat_normals = normals.is_none();
            let normals = normals.unwrap_or_else(|| vec![[0.0; 3]; count]);
            geometry
                .vertices
                .extend(positions.iter().zip(&normals).zip(&uvs).map(
                    |((&position, &normal), &uv)| Vertex {
                        position,
                        normal,
                        uv,
                    },
                ));
            if let Some(skin) = &skin {
                geometry.skin.resize(start, SkinVertex::default());
                geometry.skin.extend_from_slice(skin);
            }
            let first_index = geometry.indices.len() as u32;
            let base = start as u32;
            geometry
                .indices
                .extend(local.iter().map(|&index| base + index));
            primitives.push(Primitive {
                first_index,
                index_count: local.len() as u32,
                material: primitive.material.map(|material| material.value()),
                center: min * 0.5 + max * 0.5,
                flat_normals,
                skinned: skin.is_some(),
            });
        }
        meshes.push(primitives);
        joints_needed.push(needed);
    }
    if geometry.indices.is_empty() {
        return Err(invalid("the model has no triangles to draw"));
    }
    if !geometry.skin.is_empty() {
        geometry
            .skin
            .resize(geometry.vertices.len(), SkinVertex::default());
    }
    Ok(Meshes {
        geometry,
        meshes,
        joints_needed,
    })
}

/// The accessors of what a triangle primitive's vertices hold that the reader reads.
struct Accessors<'a> {
    positions: &'a Accessor,
    normals: Option<&'a Accessor>,
    uvs: Option<&'a Accessor>,
    /// Joints and weights.
    skin: Option<(&'a Accessor, &'a Accessor)>,
    indices: Option<&'a Accessor>,
}

/// Each vertex's joints and weights.
type SkinValues = (Vec<[u32; 4]>, Vec<[f32; 4]>);

/// A primitive's vertex attributes and indices as read, before they are checked against each
/// other.
struct Attributes {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Option<Vec<[f32; 2]>>,
    skin: Option<SkinValues>,
    indices: Option<Vec<u32>>,
}

/// Reads `accessors`, or decodes them from `primitive`'s Draco stream when it has one, which
/// declare `count` vertices and `index_count` indices.
#[cfg_attr(not(feature = "draco"), allow(unused_variables))]
fn read_attributes(
    buffers: &Buffers<'_>,
    primitive: &gltf_json::mesh::Primitive,
    accessors: &Accessors<'_>,
    count: usize,
    index_count: usize,
) -> Result<Attributes, ModelError> {
    #[cfg(feature = "draco")]
    if let Some(extension) = super::draco::extension(primitive) {
        return read_draco(buffers, extension, accessors, count, index_count);
    }
    Ok(Attributes {
        positions: buffers.vectors::<3>(accessors.positions, None)?,
        normals: accessors
            .normals
            .map(|accessor| buffers.vectors::<3>(accessor, None))
            .transpose()?,
        uvs: accessors
            .uvs
            .map(|accessor| buffers.vectors::<2>(accessor, None))
            .transpose()?,
        skin: accessors
            .skin
            .map(|(joints, weights)| -> Result<_, ModelError> {
                Ok((
                    buffers.uint_vectors::<4>(joints)?,
                    buffers.vectors::<4>(weights, Some(true))?,
                ))
            })
            .transpose()?,
        indices: accessors
            .indices
            .map(|accessor| -> Result<Vec<u32>, ModelError> {
                Ok(buffers
                    .uint_vectors::<1>(accessor)?
                    .into_iter()
                    .map(|[index]| index)
                    .collect())
            })
            .transpose()?,
    })
}

/// [`read_attributes`] for a primitive with a Draco stream. Attributes the stream doesn't hold are
/// read from their accessors.
#[cfg(feature = "draco")]
fn read_draco(
    buffers: &Buffers<'_>,
    extension: &serde_json::Value,
    accessors: &Accessors<'_>,
    count: usize,
    index_count: usize,
) -> Result<Attributes, ModelError> {
    let stream = super::draco::Stream::new(extension)?;
    // What the stream doesn't hold is read from its accessors, before anything is decoded.
    fn unless_held<'a>(
        stream: &super::draco::Stream<'_>,
        semantic: &str,
        accessor: Option<&'a Accessor>,
    ) -> Option<&'a Accessor> {
        accessor.filter(|_| !stream.holds(semantic))
    }
    let positions = unless_held(&stream, "POSITION", Some(accessors.positions))
        .map(|accessor| buffers.vectors::<3>(accessor, None))
        .transpose()?;
    let normals = unless_held(&stream, "NORMAL", accessors.normals)
        .map(|accessor| buffers.vectors::<3>(accessor, None))
        .transpose()?;
    let uvs = unless_held(&stream, "TEXCOORD_0", accessors.uvs)
        .map(|accessor| buffers.vectors::<2>(accessor, None))
        .transpose()?;
    let joints = unless_held(
        &stream,
        "JOINTS_0",
        accessors.skin.map(|(joints, _)| joints),
    )
    .map(|accessor| buffers.uint_vectors::<4>(accessor))
    .transpose()?;
    let weights = unless_held(
        &stream,
        "WEIGHTS_0",
        accessors.skin.map(|(_, weights)| weights),
    )
    .map(|accessor| buffers.vectors::<4>(accessor, Some(true)))
    .transpose()?;

    let mut read = vec![("POSITION", 3)];
    read.extend(accessors.normals.map(|_| ("NORMAL", 3)));
    read.extend(accessors.uvs.map(|_| ("TEXCOORD_0", 2)));
    if accessors.skin.is_some() {
        read.extend([("JOINTS_0", 4), ("WEIGHTS_0", 4)]);
    }
    let decoded = stream.decode(buffers, count, index_count, &read)?;
    let skin = match accessors.skin {
        Some((joints_accessor, weights_accessor)) => {
            let joints = match joints {
                Some(joints) => joints,
                None => {
                    let Checked::Valid(component) = &joints_accessor.component_type else {
                        return Err(invalid("an accessor has an unknown component type or type"));
                    };
                    if matches!(
                        component.0,
                        ComponentType::F32 | ComponentType::I8 | ComponentType::I16
                    ) {
                        return Err(invalid(
                            "an index or joint accessor doesn't hold unsigned integers",
                        ));
                    }
                    decoded
                        .vectors::<4>("JOINTS_0", joints_accessor, Some(false))?
                        .iter()
                        .map(|joint| joint.map(|index| index as u32))
                        .collect()
                }
            };
            let weights = match weights {
                Some(weights) => weights,
                None => decoded.vectors::<4>("WEIGHTS_0", weights_accessor, Some(true))?,
            };
            Some((joints, weights))
        }
        None => None,
    };
    Ok(Attributes {
        positions: match positions {
            Some(positions) => positions,
            None => decoded.vectors::<3>("POSITION", accessors.positions, None)?,
        },
        normals: match (accessors.normals, normals) {
            (Some(accessor), None) => Some(decoded.vectors::<3>("NORMAL", accessor, None)?),
            (_, normals) => normals,
        },
        uvs: match (accessors.uvs, uvs) {
            (Some(accessor), None) => Some(decoded.vectors::<2>("TEXCOORD_0", accessor, None)?),
            (_, uvs) => uvs,
        },
        skin,
        indices: Some(decoded.indices()),
    })
}

fn different_counts() -> ModelError {
    invalid("a primitive's attributes hold different numbers of vertices")
}

/// Skin vertices from joints and weights, raising `needed` to one past the highest joint any
/// vertex leans on. Joints a vertex gives no weight are joint 0, so that they never read past the
/// skin.
fn skin_vertices(
    joints: &[[u32; 4]],
    weights: &[[f32; 4]],
    needed: &mut usize,
) -> Result<Vec<SkinVertex>, ModelError> {
    let mut vertices = Vec::with_capacity(joints.len());
    for (joints, weights) in joints.iter().zip(weights) {
        let mut vertex = SkinVertex::default();
        for i in 0..4 {
            if !weights[i].is_finite() {
                return Err(invalid("a vertex's joint weight isn't a finite number"));
            }
            let weight = weights[i].clamp(0.0, 1.0);
            vertex.weights[i] = (weight * 65535.0 + 0.5) as u16;
            if vertex.weights[i] == 0 {
                continue;
            }
            let joint = joints[i] as usize;
            if joint >= MAX_SKIN_JOINTS {
                return Err(invalid("a vertex leans on a joint its skin lacks"));
            }
            vertex.joints[i] = joint as u16;
            *needed = (*needed).max(joint + 1);
        }
        vertices.push(vertex);
    }
    Ok(vertices)
}

/// UVs for a primitive without any, projected onto the plane of its two longest extents with V
/// running down the second, so that a flat screen shows a raster upright.
/// Works in `f64`, where the extent of any finite positions is finite.
fn planar_uvs(positions: &[[f32; 3]], min: Vec3, max: Vec3) -> Vec<[f32; 2]> {
    let extent = [0, 1, 2].map(|axis| (f64::from(max[axis]) - f64::from(min[axis])).max(1e-6));
    let shortest = (0..3)
        .min_by(|&a, &b| extent[a].total_cmp(&extent[b]))
        .unwrap_or(2);
    let [u, v] = match shortest {
        0 => [2, 1],
        1 => [0, 2],
        _ => [0, 1],
    };
    let along = |position: &[f32; 3], axis: usize| {
        (f64::from(position[axis]) - f64::from(min[axis])) / extent[axis]
    };
    positions
        .iter()
        .map(|position| [along(position, u) as f32, (1.0 - along(position, v)) as f32])
        .collect()
}

fn read_skins(root: &Root, buffers: &Buffers<'_>) -> Result<Vec<Skin>, ModelError> {
    let mut total = 0usize;
    let mut skins = Vec::with_capacity(root.skins.len());
    for skin in &root.skins {
        total = total.saturating_add(skin.joints.len());
        if skin.joints.len() > MAX_SKIN_JOINTS {
            return Err(too_large("joints in one skin", MAX_SKIN_JOINTS));
        }
        if total > MAX_JOINTS {
            return Err(too_large("joints", MAX_JOINTS));
        }
        let joints: Vec<usize> = skin.joints.iter().map(|joint| joint.value()).collect();
        if joints
            .iter()
            .chain(skin.skeleton.map(|skeleton| skeleton.value()).as_ref())
            .any(|&node| node >= root.nodes.len())
        {
            return Err(invalid("a skin names a node the model lacks"));
        }
        // A skin may carry more inverse bind matrices than joints; the extra ones are unused.
        let inverse_binds: Vec<Mat4> = match skin.inverse_bind_matrices {
            Some(accessor) => buffers
                .vectors::<16>(buffers.accessor(accessor)?, None)?
                .iter()
                .take(joints.len())
                .map(Mat4::from_cols_array)
                .collect(),
            None => vec![Mat4::IDENTITY; joints.len()],
        };
        if inverse_binds.len() < joints.len() {
            return Err(invalid(format!(
                "a skin has {} inverse bind matrices for its {} joints",
                inverse_binds.len(),
                joints.len()
            )));
        }
        if !inverse_binds.iter().all(Mat4::is_finite) {
            return Err(invalid("a skin's inverse bind matrix isn't finite"));
        }
        skins.push(Skin {
            joints,
            inverse_binds,
        });
    }
    Ok(skins)
}

/// The nodes and the roots of the default scene, or of the first scene when none is the default,
/// or every node without a parent when there are no scenes. Each node may have one parent at
/// most and roots none, so the hierarchy is a forest that walking from the roots can't loop in.
/// A node's matrix is kept as given unless an animation moves the node, which needs its
/// translation, rotation and scale.
fn read_nodes(
    root: &Root,
    mesh_count: usize,
    skin_count: usize,
) -> Result<(Vec<Node>, Vec<usize>), ModelError> {
    let count = root.nodes.len();
    let mut animated = vec![false; count];
    for channel in root
        .animations
        .iter()
        .flat_map(|animation| &animation.channels)
    {
        if let Some(animated) = animated.get_mut(channel.target.node.value()) {
            *animated = true;
        }
    }
    let mut nodes = Vec::with_capacity(count);
    for (index, node) in root.nodes.iter().enumerate() {
        let matrix = node.matrix.map(|matrix| Mat4::from_cols_array(&matrix));
        if matrix.is_some_and(|matrix| !matrix.is_finite()) {
            return Err(invalid(format!("node {index}'s matrix isn't finite")));
        }
        let (scale, rotation, translation) = match matrix {
            Some(matrix) if animated[index] => {
                let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
                if !(scale.is_finite() && rotation.is_finite() && translation.is_finite()) {
                    return Err(invalid(format!(
                        "node {index} is animated, but its matrix doesn't come apart into a \
                         translation, rotation and scale"
                    )));
                }
                (scale, rotation, translation)
            }
            _ => (
                node.scale.map_or(Vec3::ONE, Vec3::from),
                node.rotation
                    .map_or(Quat::IDENTITY, |rotation| Quat::from_array(rotation.0)),
                node.translation.map_or(Vec3::ZERO, Vec3::from),
            ),
        };
        if !(scale.is_finite() && rotation.is_finite() && translation.is_finite()) {
            return Err(invalid(format!("node {index}'s transform isn't finite")));
        }
        let children: Vec<usize> = node
            .children
            .iter()
            .flatten()
            .map(|child| child.value())
            .collect();
        if children.iter().any(|&child| child >= count) {
            return Err(invalid(format!("node {index} has a child the model lacks")));
        }
        let mesh = node.mesh.map(|mesh| mesh.value());
        let skin = node.skin.map(|skin| skin.value());
        if mesh.is_some_and(|mesh| mesh >= mesh_count)
            || skin.is_some_and(|skin| skin >= skin_count)
        {
            return Err(invalid(format!(
                "node {index} names a mesh or skin the model lacks"
            )));
        }
        nodes.push(Node {
            matrix: matrix.filter(|_| !animated[index]),
            translation,
            rotation: unit(rotation),
            scale,
            children,
            mesh,
            skin,
        });
    }

    let mut parents = vec![0u32; count];
    for &child in nodes.iter().flat_map(|node| &node.children) {
        parents[child] = parents[child].saturating_add(1);
    }
    let scene = match root.scene {
        Some(scene) => Some(
            root.scenes
                .get(scene.value())
                .ok_or_else(|| invalid("the default scene is one the model lacks"))?,
        ),
        None => root.scenes.first(),
    };
    let roots: Vec<usize> = match scene {
        Some(scene) => scene.nodes.iter().map(|node| node.value()).collect(),
        None => (0..count).filter(|&node| parents[node] == 0).collect(),
    };
    for &root in &roots {
        let parent = parents
            .get_mut(root)
            .ok_or_else(|| invalid("a scene names a node the model lacks"))?;
        *parent = parent.saturating_add(1);
    }
    if parents.iter().any(|&count| count > 1) {
        return Err(invalid(
            "the model's nodes don't form a tree: a node has two parents, or is its own ancestor",
        ));
    }
    Ok((nodes, roots))
}

/// Which nodes the scene reaches from `roots`, which `read_nodes` checked form a forest.
fn reached(nodes: &[Node], roots: &[usize]) -> Vec<bool> {
    let mut reached = vec![false; nodes.len()];
    let mut pending = roots.to_vec();
    while let Some(index) = pending.pop() {
        reached[index] = true;
        pending.extend(&nodes[index].children);
    }
    reached
}

/// How many primitives the nodes the scene reaches place.
fn count_draws(
    nodes: &[Node],
    reached: &[bool],
    meshes: &[Vec<Primitive>],
) -> Result<usize, ModelError> {
    let mut draws = 0usize;
    for (node, _) in nodes.iter().zip(reached).filter(|(_, reached)| **reached) {
        if let Some(mesh) = node.mesh {
            draws = draws.saturating_add(meshes[mesh].len());
            if draws > MAX_DRAWS {
                return Err(too_large("primitives placed in its scene", MAX_DRAWS));
            }
        }
    }
    Ok(draws)
}

/// `value`, which a material factor must hold as a finite number.
fn factor(value: f32) -> Result<f32, ModelError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(invalid("a material's factor isn't a finite number"))
    }
}

/// Where an image's bytes are: a range of a buffer, which any number of images and buffer views
/// may share, or the data URI of the image at this index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ImageBytes {
    Range(ByteRange),
    DataUri(usize),
}

/// A material slot's texture before images are decoded: where its image's bytes are, and whether
/// the slot reads them as sRGB colour.
type Pending = (ImageBytes, bool);

/// The map a material slot reads `info` through, or `None` when it reads none, or reads a UV set
/// other than the first, which isn't supported.
fn pending_map(
    root: &Root,
    buffers: &Buffers<'_>,
    info: Option<(Index<gltf_json::Texture>, u32)>,
    srgb: bool,
) -> Result<Option<Map<Pending>>, ModelError> {
    let Some((texture, tex_coord)) = info else {
        return Ok(None);
    };
    if tex_coord != 0 {
        return Ok(None);
    }
    let json = root
        .textures
        .get(texture.value())
        .ok_or_else(|| invalid("a material names a texture the model lacks"))?;
    let sampler = match json.sampler {
        Some(sampler) => sampler_of(
            root.samplers
                .get(sampler.value())
                .ok_or_else(|| invalid("a texture names a sampler the model lacks"))?,
        ),
        None => Sampler::default(),
    };
    let image = texture_source(json)?;
    let bytes = match root.images.get(image) {
        Some(gltf_json::Image {
            buffer_view: Some(view),
            ..
        }) => ImageBytes::Range(buffers.view_range(*view)?),
        Some(gltf_json::Image { uri: Some(_), .. }) => ImageBytes::DataUri(image),
        Some(_) => return Err(invalid("an image in the model has no data")),
        None => return Err(invalid("a texture names an image the model lacks")),
    };
    Ok(Some(Map {
        texture: (bytes, srgb),
        sampler,
    }))
}

/// The textures `pending` names, in order of first use. Each image's bytes are read once however
/// many images, buffer views and slots name them. Before any is read, the distinct images' encoded
/// bytes together are checked against the `file_bytes` the file holds, and every texture's mip
/// levels against [`MAX_TEXTURE_PIXELS`]; then each image is decoded in turn, with only its own
/// bytes copied.
fn decode_textures(
    root: &Root,
    buffers: &Buffers<'_>,
    file_bytes: usize,
    pending: &[Pending],
) -> Result<(Vec<Texture>, HashMap<Pending, usize>), ModelError> {
    let mut order: Vec<Pending> = Vec::new();
    let mut indices = HashMap::new();
    for &texture in pending {
        indices.entry(texture).or_insert_with(|| {
            order.push(texture);
            order.len() - 1
        });
    }
    // Each image's bytes in order of first use, with the textures read from them.
    let mut images: Vec<(ImageBytes, Vec<usize>)> = Vec::new();
    let mut image_of = HashMap::new();
    for (texture, &(bytes, _)) in order.iter().enumerate() {
        let image = *image_of.entry(bytes).or_insert_with(|| {
            images.push((bytes, Vec::new()));
            images.len() - 1
        });
        images[image].1.push(texture);
    }
    let uri = |image: usize| root.images[image].uri.as_deref().unwrap_or_default();
    let mut encoded = 0usize;
    for &(bytes, _) in &images {
        let length = match bytes {
            ImageBytes::Range(range) => range.length,
            ImageBytes::DataUri(image) => data_uri_length(uri(image))?,
        };
        encoded = encoded.saturating_add(length);
        if encoded > file_bytes {
            return Err(ModelError::TooLarge(format!(
                "the model's images take more than the {file_bytes} bytes of its file"
            )));
        }
    }
    let data = |bytes: ImageBytes| -> Result<Cow<'_, [u8]>, ModelError> {
        Ok(match bytes {
            ImageBytes::Range(range) => Cow::Borrowed(buffers.bytes(range)),
            ImageBytes::DataUri(image) => Cow::Owned(data_uri(uri(image))?),
        })
    };
    let mut headers = Vec::with_capacity(images.len());
    let mut pixels = 0u64;
    let mut expansion = image::Expansion::default();
    for (bytes, textures) in &images {
        let data = data(*bytes)?;
        let header = image::header(&data)?;
        expansion.add(header, data.len());
        pixels = image::mip_pixels(header.width, header.height)
            .checked_mul(textures.len() as u64)
            .and_then(|charge| pixels.checked_add(charge))
            .filter(|&total| total <= MAX_TEXTURE_PIXELS)
            .ok_or_else(|| too_large("texture pixels", MAX_TEXTURE_PIXELS))?;
        headers.push(header);
    }
    expansion.check()?;

    let mut textures: Vec<Option<Texture>> = vec![None; order.len()];
    for ((bytes, uses), &header) in images.iter().zip(&headers) {
        let mut levels = image::decode(&data(*bytes)?, header)?;
        for (count, &texture) in uses.iter().enumerate() {
            // The last texture read from the image takes the decoded pixels themselves.
            let pixels = if count + 1 == uses.len() {
                std::mem::take(&mut levels)
            } else {
                levels.clone()
            };
            let srgb = order[texture].1;
            textures[texture] = Some(image::texture(pixels, header.width, header.height, srgb));
        }
    }
    Ok((textures.into_iter().flatten().collect(), indices))
}

fn sampler_of(sampler: &gltf_json::texture::Sampler) -> Sampler {
    let wrap = |mode: &Checked<WrappingMode>| match mode {
        Checked::Valid(WrappingMode::ClampToEdge) => Wrap::ClampToEdge,
        Checked::Valid(WrappingMode::MirroredRepeat) => Wrap::MirroredRepeat,
        _ => Wrap::Repeat,
    };
    let mag = match sampler.mag_filter {
        Some(Checked::Valid(MagFilter::Nearest)) => Filter::Nearest,
        _ => Filter::Linear,
    };
    let (min, mipmap) = match sampler.min_filter {
        Some(Checked::Valid(MinFilter::Nearest)) => (Filter::Nearest, None),
        Some(Checked::Valid(MinFilter::Linear)) => (Filter::Linear, None),
        Some(Checked::Valid(MinFilter::NearestMipmapNearest)) => {
            (Filter::Nearest, Some(Filter::Nearest))
        }
        Some(Checked::Valid(MinFilter::LinearMipmapNearest)) => {
            (Filter::Linear, Some(Filter::Nearest))
        }
        Some(Checked::Valid(MinFilter::NearestMipmapLinear)) => {
            (Filter::Nearest, Some(Filter::Linear))
        }
        _ => (Filter::Linear, Some(Filter::Linear)),
    };
    Sampler {
        wrap_u: wrap(&sampler.wrap_s),
        wrap_v: wrap(&sampler.wrap_t),
        mag,
        min,
        mipmap,
    }
}

fn read_materials(
    root: &Root,
    buffers: &Buffers<'_>,
    file_bytes: usize,
) -> Result<(Vec<Material>, Vec<Texture>), ModelError> {
    let mut materials = Vec::with_capacity(root.materials.len());
    let mut maps = Vec::with_capacity(root.materials.len());
    for material in &root.materials {
        let pbr = &material.pbr_metallic_roughness;
        let extensions = material.extensions.as_ref();
        let strength = match extensions.and_then(|extensions| extensions.emissive_strength.as_ref())
        {
            Some(strength) => factor(strength.emissive_strength.0)?.max(0.0),
            None => 1.0,
        };
        let info = |info: &Option<gltf_json::texture::Info>| {
            info.as_ref().map(|info| (info.index, info.tex_coord))
        };
        let blending = match material.alpha_mode {
            Checked::Valid(AlphaMode::Mask) => Blending::Mask {
                cutoff: factor(material.alpha_cutoff.map_or(0.5, |cutoff| cutoff.0))?,
            },
            Checked::Valid(AlphaMode::Blend) => Blending::Blend,
            _ => Blending::Opaque,
        };
        let [r, g, b, a] = pbr.base_color_factor.0;
        let base_color = [factor(r)?, factor(g)?, factor(b)?, factor(a)?];
        let [r, g, b] = material.emissive_factor.0;
        let emissive = [factor(r)?, factor(g)?, factor(b)?];
        materials.push(Material {
            name: material.name.clone().unwrap_or_default(),
            base_color: base_color.map(|channel| channel.clamp(0.0, 1.0)),
            metallic: factor(pbr.metallic_factor.0)?.clamp(0.0, 1.0),
            roughness: factor(pbr.roughness_factor.0)?.clamp(0.0, 1.0),
            emissive: emissive.map(|channel| channel.max(0.0) * strength),
            normal_scale: match &material.normal_texture {
                Some(normal) => factor(normal.scale)?,
                None => 1.0,
            },
            occlusion_strength: match &material.occlusion_texture {
                Some(occlusion) => factor(occlusion.strength.0)?.clamp(0.0, 1.0),
                None => 1.0,
            },
            blending,
            unlit: extensions.is_some_and(|extensions| extensions.unlit.is_some()),
            double_sided: material.double_sided,
            maps: Maps::default(),
        });
        maps.push(Maps {
            base_color: pending_map(root, buffers, info(&pbr.base_color_texture), true)?,
            metallic_roughness: pending_map(
                root,
                buffers,
                info(&pbr.metallic_roughness_texture),
                false,
            )?,
            normal: pending_map(
                root,
                buffers,
                material
                    .normal_texture
                    .as_ref()
                    .map(|normal| (normal.index, normal.tex_coord)),
                false,
            )?,
            emissive: pending_map(root, buffers, info(&material.emissive_texture), true)?,
            occlusion: pending_map(
                root,
                buffers,
                material
                    .occlusion_texture
                    .as_ref()
                    .map(|occlusion| (occlusion.index, occlusion.tex_coord)),
                false,
            )?,
        });
    }
    let pending: Vec<Pending> = maps
        .iter()
        .flat_map(|maps: &Maps<Pending>| {
            [
                maps.base_color,
                maps.metallic_roughness,
                maps.normal,
                maps.emissive,
                maps.occlusion,
            ]
        })
        .flatten()
        .map(|map| map.texture)
        .collect();
    let (textures, indices) = decode_textures(root, buffers, file_bytes, &pending)?;
    for (material, maps) in materials.iter_mut().zip(&maps) {
        material.maps = maps.map(|texture| indices[texture]);
    }
    Ok((materials, textures))
}

fn read_animations(
    root: &Root,
    buffers: &Buffers<'_>,
    node_count: usize,
) -> Result<Vec<Animation>, ModelError> {
    let mut animations = Vec::with_capacity(root.animations.len());
    for animation in &root.animations {
        let mut duration = 0.0f32;
        let mut channels = Vec::new();
        for channel in &animation.channels {
            let sampler = animation
                .samplers
                .get(channel.sampler.value())
                .ok_or_else(|| invalid("an animation channel names a sampler it lacks"))?;
            let node = channel.target.node.value();
            if node >= node_count {
                return Err(invalid("an animation targets a node the model lacks"));
            }
            let interpolation = match sampler.interpolation {
                Checked::Valid(JsonInterpolation::Step) => Interpolation::Step,
                Checked::Valid(JsonInterpolation::CubicSpline) => Interpolation::CubicSpline,
                _ => Interpolation::Linear,
            };
            let property = match channel.target.path {
                Checked::Valid(
                    property @ (Property::Translation | Property::Rotation | Property::Scale),
                ) => property,
                _ => continue,
            };
            let input = buffers.accessor(sampler.input)?;
            let times = buffers.vectors::<1>(input, None)?;
            let times: Vec<f32> = times.into_iter().map(|[time]| time).collect();
            if times.iter().any(|time| !time.is_finite())
                || times.windows(2).any(|pair| pair[1] < pair[0])
            {
                return Err(invalid(
                    "an animation's key times aren't finite numbers in order",
                ));
            }
            let Some(&last) = times.last() else {
                continue;
            };
            let output = buffers.accessor(sampler.output)?;
            let cubic = interpolation == Interpolation::CubicSpline;
            let values = match property {
                // Rotations are normalised, but a cubic spline's tangents, the first and last of
                // each three values, are not.
                Property::Rotation => Values::Rotation(
                    buffers
                        .vectors::<4>(output, None)?
                        .into_iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let quat = Quat::from_array(value);
                            if !quat.is_finite() || (cubic && index % 3 != 1) {
                                quat
                            } else {
                                unit(quat)
                            }
                        })
                        .collect(),
                ),
                Property::Translation => Values::Translation(
                    buffers
                        .vectors::<3>(output, None)?
                        .into_iter()
                        .map(Vec3::from)
                        .collect(),
                ),
                _ => Values::Scale(
                    buffers
                        .vectors::<3>(output, None)?
                        .into_iter()
                        .map(Vec3::from)
                        .collect(),
                ),
            };
            let (count, finite) = match &values {
                Values::Translation(values) | Values::Scale(values) => {
                    (values.len(), values.iter().all(|value| value.is_finite()))
                }
                Values::Rotation(values) => {
                    (values.len(), values.iter().all(|value| value.is_finite()))
                }
            };
            let per_key = if cubic { 3 } else { 1 };
            if Some(count) != times.len().checked_mul(per_key) {
                return Err(invalid(
                    "an animation has a different number of key times and values",
                ));
            }
            if !finite {
                return Err(invalid("an animation's values aren't finite numbers"));
            }
            duration = duration.max(last);
            channels.push(Channel {
                node,
                interpolation,
                times,
                values,
            });
        }
        animations.push(Animation {
            name: animation.name.clone().unwrap_or_default(),
            duration,
            channels,
        });
    }
    Ok(animations)
}
