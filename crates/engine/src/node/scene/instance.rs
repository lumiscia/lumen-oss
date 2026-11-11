//! `instance_3d`: many placed copies of one scene.

use serde_json::Value;

use super::Placement3d;
use crate::error::LumenError;
use crate::expr::ExpressionValue;
use crate::node::{
    Deferred, DelegateEvaluable, Delegated, NodeId, NodeParamType, NodeParams, ParamValue,
    PropertyKind, truncated_int,
};

/// Most copies an `instance_3d` may place. It is also the most objects a frame's scene may
/// draw, so a single copy of each of that many objects fills it.
pub const MAX_INSTANCES: u32 = 100_000;

/// How many copies an `instance_3d` places, from 0 to [`MAX_INSTANCES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::StructuralHash)]
pub struct InstanceCount(pub u32);

impl InstanceCount {
    fn new(count: i64) -> Option<Self> {
        u32::try_from(count)
            .ok()
            .filter(|count| *count <= MAX_INSTANCES)
            .map(Self)
    }
}

impl ParamValue for InstanceCount {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let count = i64::from_json_value(value, path)?;
        Self::new(count).ok_or_else(|| {
            format!("`{path}` must be an integer from 0 to {MAX_INSTANCES}, got {count}")
        })
    }

    fn to_json_value(&self) -> Value {
        Value::from(self.0)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        let count = truncated_int(value)?;
        Self::new(count).ok_or_else(|| {
            LumenError::evaluate(format!(
                "{count} instances is out of range; an instance_3d places 0 to {MAX_INSTANCES}"
            ))
        })
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(f64::from(self.0)))
    }
}

impl Delegated for InstanceCount {
    type Delegate = Deferred<InstanceCount>;

    fn into_delegate(self) -> Self::Delegate {
        Deferred::value(self)
    }
}

impl NodeParamType for InstanceCount {
    fn property_kind() -> PropertyKind {
        PropertyKind::Int
    }
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Instance3dParams {
    /// Copies to place, from 0 to 100000. 0 leaves the node's scene out entirely, lights and
    /// cameras too.
    #[meta(min = 0, enforced_min, max = 100000, step = 1)]
    pub count: InstanceCount,
    #[meta(
        flatten,
        doc(
            position = "Position in world units along X, Y and Z of each copy, in the scene's own space. Evaluated for every copy, and can read `instance` and `instance_count`.",
            rotation = "Rotation in degrees about X, Y and Z of each copy, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z. Evaluated for every copy, and can read `instance` and `instance_count`.",
            scale = "Scale multiplier along X, Y and Z of each copy, applied before the rotation. Evaluated for every copy, and can read `instance` and `instance_count`."
        )
    )]
    pub placement: Placement3d,
}

impl Default for Instance3dParams {
    fn default() -> Self {
        Self {
            count: InstanceCount(1),
            placement: Placement3d::default(),
        }
    }
}

/// Draws its input scene `count` times, each copy placed by `position`, `rotation` and `scale`
/// evaluated for that copy, where `instance` is the copy's index from 0 and `instance_count` is
/// `count`: `position = vec3((instance % 10) * 1.5, 0, floor(instance / 10) * 1.5)` lays out a
/// grid, and a ring, a spiral or a path follow from `sin`, `cos` and `path_point` of
/// `instance / instance_count`. A copy is placed in the input scene's own space, so it turns and
/// scales about the origin of the scene's own coordinates.
///
/// Copies share the input's meshes, textures, materials and joint matrices on the GPU, so every
/// copy of a skinned model plays the same animation at the same time. Only the objects are copied:
/// the input's lights and camera appear once, placed as they would be without the node, so
/// copies don't multiply a scene's lighting. Shadows, fog and the environment treat a copy as any
/// other object. An `instance_3d` may hold another, which multiplies their counts, up to 8 deep.
/// A scene draws at most 100000 objects in all, counting each copy's objects, and a count that
/// would exceed this fails the render. The node is static when its params don't read the frame
/// and its scene is static.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "instance_3d",
    name = "Instance 3D",
    category = "scene",
    output = "scene"
)]
pub struct Instance3d {
    pub id: NodeId,
    #[params]
    pub params: Instance3dParamsDelegate,

    #[input(scene)]
    pub scene: Option<NodeId>,
}

impl Instance3d {
    /// Whether its params give the same copies at every frame, so that only the instance index
    /// and count vary.
    pub(crate) fn is_frame_independent(&self) -> bool {
        let mut independent = true;
        self.params
            .for_each_expression(&mut |expression| independent &= !expression.is_frame_dependent());
        independent
    }
}
