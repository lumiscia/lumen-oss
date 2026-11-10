use std::collections::{HashMap, HashSet};

use glam::Vec3;

use super::{Geometry, ModelScene, Primitive};
use crate::Bounds;

/// What the vertices of every primitive a skin moves cover before posing.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinBounds {
    /// By joint, the vertices it moves, empty for a joint that moves none. A posed vertex lies
    /// within these bounds moved by its joints' matrices.
    pub joints: Vec<Bounds>,
    /// The vertices no joint moves, which are drawn where they are.
    pub unmoved: Bounds,
}

impl ModelScene {
    /// The bounds of each skin's vertices, before the joint matrices
    /// [`Pose::skins`](super::Pose::skins) finds move them.
    pub fn joint_bounds(&self, geometry: &Geometry) -> Vec<SkinBounds> {
        let mut skins: Vec<SkinBounds> = self
            .skins
            .iter()
            .map(|skin| SkinBounds {
                joints: vec![Bounds::EMPTY; skin.joints.len()],
                unmoved: Bounds::EMPTY,
            })
            .collect();
        let mut by_primitive: HashMap<(u32, u32), Moved> = HashMap::new();
        let mut merged = HashSet::new();
        for node in &self.nodes {
            let (Some(skin), Some(mesh)) = (node.skin, node.mesh) else {
                continue;
            };
            let (Some(bounds), Some(primitives)) = (skins.get_mut(skin), self.meshes.get(mesh))
            else {
                continue;
            };
            for primitive in primitives.iter().filter(|primitive| primitive.skinned) {
                let key = (primitive.first_index, primitive.index_count);
                if !merged.insert((skin, key)) {
                    continue;
                }
                let moved = by_primitive
                    .entry(key)
                    .or_insert_with(|| moved_by_joint(geometry, primitive));
                for &(joint, moved) in &moved.joints {
                    if let Some(joint) = bounds.joints.get_mut(joint) {
                        *joint = joint.union(moved);
                    }
                }
                bounds.unmoved = bounds.unmoved.union(moved.unmoved);
            }
        }
        skins
    }
}

/// The bounds of one primitive's vertices that each joint moves, for the joints that move any, and
/// of those none moves.
struct Moved {
    joints: Vec<(usize, Bounds)>,
    unmoved: Bounds,
}

fn moved_by_joint(geometry: &Geometry, primitive: &Primitive) -> Moved {
    let start = primitive.first_index as usize;
    let indices = geometry
        .indices
        .get(start..start + primitive.index_count as usize)
        .unwrap_or_default();
    let mut joints: Vec<Bounds> = Vec::new();
    let mut unmoved = Bounds::EMPTY;
    for &index in indices {
        let (Some(vertex), Some(skin)) = (
            geometry.vertices.get(index as usize),
            geometry.skin.get(index as usize),
        ) else {
            continue;
        };
        let position = Vec3::from(vertex.position);
        if skin.weights == [0; 4] {
            unmoved = unmoved.union(Bounds::of([position]));
        }
        for (&joint, &weight) in skin.joints.iter().zip(&skin.weights) {
            if weight == 0 {
                continue;
            }
            let joint = joint as usize;
            if joints.len() <= joint {
                joints.resize(joint + 1, Bounds::EMPTY);
            }
            joints[joint] = joints[joint].union(Bounds::of([position]));
        }
    }
    let joints = joints
        .into_iter()
        .enumerate()
        .filter(|(_, bounds)| !bounds.is_empty())
        .collect();
    Moved { joints, unmoved }
}
