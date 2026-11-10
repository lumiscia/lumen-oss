use std::collections::{HashMap, HashSet};

use glam::Vec3;

use super::{Geometry, ModelScene, Primitive};
use crate::Bounds;

impl ModelScene {
    /// For each skin, by joint, the bounds of the vertices that joint moves in every primitive the
    /// skin moves, before the joint matrices move them. A posed skinned vertex lies within these
    /// bounds moved by its joints' matrices, which [`Pose::skins`](super::Pose::skins) finds.
    /// Joints that move no vertex have empty bounds.
    pub fn joint_bounds(&self, geometry: &Geometry) -> Vec<Vec<Bounds>> {
        let mut skins: Vec<Vec<Bounds>> = self
            .skins
            .iter()
            .map(|skin| vec![Bounds::EMPTY; skin.joints.len()])
            .collect();
        let mut by_primitive: HashMap<(u32, u32), Vec<(usize, Bounds)>> = HashMap::new();
        let mut merged = HashSet::new();
        for node in &self.nodes {
            let (Some(skin), Some(mesh)) = (node.skin, node.mesh) else {
                continue;
            };
            let (Some(joints), Some(primitives)) = (skins.get_mut(skin), self.meshes.get(mesh))
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
                for &(joint, bounds) in moved.iter() {
                    if let Some(joint) = joints.get_mut(joint) {
                        *joint = joint.union(bounds);
                    }
                }
            }
        }
        skins
    }
}

/// The bounds of `primitive`'s vertices that each joint moves, for the joints that move any.
fn moved_by_joint(geometry: &Geometry, primitive: &Primitive) -> Vec<(usize, Bounds)> {
    let start = primitive.first_index as usize;
    let indices = geometry
        .indices
        .get(start..start + primitive.index_count as usize)
        .unwrap_or_default();
    let mut joints: Vec<Bounds> = Vec::new();
    for &index in indices {
        let (Some(vertex), Some(skin)) = (
            geometry.vertices.get(index as usize),
            geometry.skin.get(index as usize),
        ) else {
            continue;
        };
        let position = Vec3::from(vertex.position);
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
    joints
        .into_iter()
        .enumerate()
        .filter(|(_, bounds)| !bounds.is_empty())
        .collect()
}
