//! `instance_3d`: many placed copies of one scene.

use serde_json::{Map, Value};

use super::{InstanceCopies, Placement3d};
use crate::error::LumenError;
use crate::expr::ExpressionValue;
use crate::node::vector::paint::Paint;
use crate::node::{
    Deferred, Delegated, NodeId, NodeParamType, NodeParams, OutputRef, ParamValue, PropertyKind,
    truncated_int,
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
    /// cameras too. Not set together with `copies`.
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
    /// Colour that multiplies the base colour of every object in each copy, texture and base
    /// colour map included, and the light they give off, so a glowing copy glows in its tint.
    /// White leaves a copy as it is. Evaluated for every copy, and can read `instance` and
    /// `instance_count`. Its alpha is ignored: a copy is opaque or blended exactly as its
    /// material makes it. Tints of nested `instance_3d` nodes multiply. Must be a solid color; a
    /// gradient fails `check`.
    #[meta(role = "color", solid)]
    pub tint: Paint,
    /// Copies to place, each given literally as `{ position, rotation, scale, tint }` with any
    /// field left out taking the default of the param of that name: position and rotation 0,
    /// scale 1 and a white tint. The list can't hold expressions and holds at most 100000
    /// copies. An empty list places none of its own. When it has copies, `count`, `position`,
    /// `rotation`, `scale` and `tint` must not be set, and `check` fails if they are.
    #[meta()]
    pub copies: InstanceCopies,
}

impl Default for Instance3dParams {
    fn default() -> Self {
        Self {
            count: InstanceCount(1),
            placement: Placement3d::default(),
            tint: Paint::solid([255, 255, 255, 255]),
            copies: InstanceCopies::default(),
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
/// `tint` colours each copy: `tint = rgb(255 * instance / instance_count, 128, 255)` shifts the
/// colour along the copies, and it multiplies the objects' base colours and emissive light, so
/// copies of a white or textured object take the tint's colour. Its alpha is ignored.
///
/// Copies placed by hand are listed in `copies` instead, as literals: `copies = [{ "position":
/// [0, 0, 0], "scale": [1, 2, 1], "tint": "#ff8040" }, { "position": [2, 0, 1], "rotation":
/// [0, 45, 0] }]` places two. `count`, `position`, `rotation`, `scale` and `tint` must not be set
/// beside a `copies` list that has copies, or `check` fails. A list is placed once, so a frame
/// after the first does no work per copy.
///
/// Copies share the input's meshes, textures, materials and joint matrices on the GPU, so every
/// copy of a skinned model plays the same animation at the same time. Opaque surfaces draw all
/// their copies at once from placements and tints kept on the GPU, so a moving camera or parent
/// costs nothing per copy, and neither does a frame when the params don't read the frame. Blended
/// surfaces, and those showing a raster, are copied one by one, which costs more. Only the objects
/// are copied: the input's lights and camera appear once, placed as they would be without the node,
/// so copies don't multiply a scene's lighting. Shadows, fog and the environment treat a copy as any
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
    pub scene: Option<OutputRef>,
}

impl Instance3d {
    /// What is wrong with a node whose JSON `params` set `copies` beside params that it replaces,
    /// if anything is.
    pub(crate) fn copies_conflict(params: &Map<String, Value>) -> Option<String> {
        let has_copies = params
            .get("copies")
            .and_then(Value::as_array)
            .is_some_and(|copies| !copies.is_empty());
        if !has_copies {
            return None;
        }
        let set: Vec<String> = ["count", "position", "rotation", "scale", "tint"]
            .into_iter()
            .filter(|name| params.contains_key(*name))
            .map(|name| format!("`{name}`"))
            .collect();
        (!set.is_empty()).then(|| {
            format!(
                "`copies` places the copies itself, so it can't be set together with {}",
                set.join(", ")
            )
        })
    }
}
