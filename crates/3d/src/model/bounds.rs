use std::collections::{HashMap, HashSet};

use glam::Vec3;

use super::{Geometry, ModelScene, Primitive};
use crate::Bounds;

/// What vertices cover: `still` without their morph targets, and `spread` the most their targets
/// move any of them along each axis for each unit of the largest weight they're given.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reach {
    pub still: Bounds,
    pub spread: Vec3,
}

impl Reach {
    pub const EMPTY: Self = Self {
        still: Bounds::EMPTY,
        spread: Vec3::ZERO,
    };

    pub fn union(self, other: Self) -> Self {
        Self {
            still: self.still.union(other.still),
            spread: self.spread.max(other.spread),
        }
    }

    /// What the vertices cover with no morph target weight past `weight` either way.
    pub fn at(&self, weight: f32) -> Bounds {
        if self.still.is_empty() {
            return Bounds::EMPTY;
        }
        let spread = self.spread * weight.abs();
        Bounds {
            min: self.still.min - spread,
            max: self.still.max + spread,
        }
    }
}

/// What the vertices of every primitive a skin moves cover before posing.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinBounds {
    /// By joint, the vertices it moves, empty for a joint that moves none. A posed vertex lies
    /// within these bounds, at its weights, moved by its joints' matrices.
    pub joints: Vec<Reach>,
    /// The vertices no joint moves, which are drawn where they are.
    pub unmoved: Reach,
}

impl ModelScene {
    /// The reach of the vertices each primitive draws, by its first index and index count.
    pub fn primitive_bounds(&self, geometry: &Geometry) -> HashMap<(u32, u32), Reach> {
        self.meshes
            .iter()
            .flatten()
            .map(|primitive| {
                let reach = drawn(geometry, primitive)
                    .iter()
                    .filter_map(|&index| geometry.reach(index))
                    .fold(Reach::EMPTY, Reach::union);
                ((primitive.first_index, primitive.index_count), reach)
            })
            .collect()
    }

    /// The bounds of each skin's vertices, before the joint matrices
    /// [`Pose::skins`](super::Pose::skins) finds move them.
    pub fn joint_bounds(&self, geometry: &Geometry) -> Vec<SkinBounds> {
        let mut skins: Vec<SkinBounds> = self
            .skins
            .iter()
            .map(|skin| SkinBounds {
                joints: vec![Reach::EMPTY; skin.joints.len()],
                unmoved: Reach::EMPTY,
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

/// The indices `primitive` draws.
fn drawn<'a>(geometry: &'a Geometry, primitive: &Primitive) -> &'a [u32] {
    let start = primitive.first_index as usize;
    geometry
        .indices
        .get(start..start + primitive.index_count as usize)
        .unwrap_or_default()
}

/// The reach of one primitive's vertices that each joint moves, for the joints that move any, and
/// of those none moves.
struct Moved {
    joints: Vec<(usize, Reach)>,
    unmoved: Reach,
}

fn moved_by_joint(geometry: &Geometry, primitive: &Primitive) -> Moved {
    let mut joints: Vec<Reach> = Vec::new();
    let mut unmoved = Reach::EMPTY;
    for &index in drawn(geometry, primitive) {
        let (Some(reach), Some(skin)) = (geometry.reach(index), geometry.skin.get(index as usize))
        else {
            continue;
        };
        if skin.weights == [0; 4] {
            unmoved = unmoved.union(reach);
        }
        for (&joint, &weight) in skin.joints.iter().zip(&skin.weights) {
            if weight == 0 {
                continue;
            }
            let joint = joint as usize;
            if joints.len() <= joint {
                joints.resize(joint + 1, Reach::EMPTY);
            }
            joints[joint] = joints[joint].union(reach);
        }
    }
    let joints = joints
        .into_iter()
        .enumerate()
        .filter(|(_, reach)| !reach.still.is_empty())
        .collect();
    Moved { joints, unmoved }
}
