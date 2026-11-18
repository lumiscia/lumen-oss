//! `particles_3d`: copies of a scene scattered over a surface and emitted from it over time.

use serde_json::Value;

use super::Pose3d;
use crate::error::LumenError;
use crate::expr::ExpressionValue;
use crate::node::{
    Deferred, Delegated, NodeId, NodeParamType, NodeParams, ParamValue, PropertyKind, truncated_int,
};

/// Most particles a `particles_3d` draws.
pub const MAX_PARTICLES: u32 = 100_000;

/// How many particles a `particles_3d` draws, from 0 to [`MAX_PARTICLES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::StructuralHash)]
pub struct ParticleCount(pub u32);

impl ParticleCount {
    fn new(count: i64) -> Option<Self> {
        u32::try_from(count)
            .ok()
            .filter(|count| *count <= MAX_PARTICLES)
            .map(Self)
    }
}

impl ParamValue for ParticleCount {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let count = i64::from_json_value(value, path)?;
        Self::new(count).ok_or_else(|| {
            format!("`{path}` must be an integer from 0 to {MAX_PARTICLES}, got {count}")
        })
    }

    fn to_json_value(&self) -> Value {
        Value::from(self.0)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        let count = truncated_int(value)?;
        Self::new(count).ok_or_else(|| {
            LumenError::evaluate(format!(
                "{count} particles is out of range; a particles_3d draws 0 to {MAX_PARTICLES}"
            ))
        })
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(f64::from(self.0)))
    }
}

impl Delegated for ParticleCount {
    type Delegate = Deferred<ParticleCount>;

    fn into_delegate(self) -> Self::Delegate {
        Deferred::value(self)
    }
}

impl NodeParamType for ParticleCount {
    fn property_kind() -> PropertyKind {
        PropertyKind::Int
    }
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Particles3dParams {
    /// Particles, from 0 to 100000. Each is one copy of `object`. With `rate` above 0, they are
    /// slots that are born one after another and born again once all have been.
    #[meta(min = 0, enforced_min, max = 100000, step = 1)]
    pub count: ParticleCount,
    /// Picks an unrelated scatter: where each particle is emitted, and its direction, size and
    /// turn. The same seed gives the same particles at the same time.
    #[meta(step = 1)]
    pub seed: i64,
    /// Particles born per second: particle `k` is born `k / rate` seconds into the composition,
    /// and born again, somewhere new, every `count / rate` seconds after. 0 gives birth to all of
    /// them at once, at 0 seconds, and never again.
    #[meta(min = 0, step = 1)]
    pub rate: f64,
    /// Seconds a particle lives once born, after which it draws nothing until it is born again.
    /// 0 lives until it is born again, or forever when `rate` is 0. With `rate` above 0, a
    /// particle is born again every `count / rate` seconds, which cuts a longer lifetime short.
    #[meta(min = 0, step = 0.1)]
    pub lifetime: f64,
    /// World units per second a particle moves at, along its emit direction, from where it is
    /// emitted.
    #[meta(min = 0, step = 0.1)]
    pub speed: f64,
    /// Degrees the emit direction may lean from the surface normal, or from +Y without a surface,
    /// at random within a cone: 0 emits along the normal and 180 every way.
    #[meta(min = 0, max = 180, step = 1)]
    pub spread: f64,
    /// Acceleration in world units per second squared along X, Y and Z, in the node's own space:
    /// `[0, -9.8, 0]` makes particles fall. A particle is at `emit + velocity * age + gravity *
    /// age² / 2`.
    #[meta(step = 0.1)]
    pub gravity: [f64; 3],
    /// Turns each particle so that its +Y follows the normal of the surface where it was emitted.
    /// Off, particles keep the node's axes.
    #[meta()]
    pub align_to_normal: bool,
    /// The smallest and largest uniform scale of a particle, each picked at random between them.
    /// Negative scales are taken as 0.
    #[meta(step = 0.1)]
    pub scale: (f64, f64),
    /// Most degrees a particle turns about an axis picked at random, by an angle picked at random
    /// up to this. 180 turns particles every way.
    #[meta(min = 0, max = 180, step = 1)]
    pub rotation_jitter: f64,
    /// The scale multiplier at a particle's birth and at the end of its `lifetime`, between which
    /// it moves in a straight line with its age. Fade particles out with an end of 0. With a
    /// `lifetime` of 0 the start applies throughout.
    #[meta(step = 0.1)]
    pub scale_over_life: (f64, f64),
    #[meta(
        flatten,
        doc(
            position = "Position in world units along X, Y and Z of the whole system in the parent's space: the surface, the particles and, without a surface, the point they are emitted from move with it.",
            rotation = "Rotation in degrees about X, Y and Z of the whole system, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z. It turns `gravity`, and the direction particles are emitted along without a surface, with the rest."
        )
    )]
    pub pose: Pose3d,
}

impl Default for Particles3dParams {
    fn default() -> Self {
        Self {
            count: ParticleCount(1000),
            seed: 0,
            rate: 0.0,
            lifetime: 0.0,
            speed: 0.0,
            spread: 0.0,
            gravity: [0.0; 3],
            align_to_normal: false,
            scale: (1.0, 1.0),
            rotation_jitter: 0.0,
            scale_over_life: (1.0, 1.0),
            pose: Pose3d::default(),
        }
    }
}

/// Draws `count` copies of its `object` scene as particles, scattered over its `surface` scene:
/// each is emitted from a point picked at random over the surface's triangles, as likely
/// anywhere as anywhere else of the same area, then moves along a direction near the
/// triangle's normal with `speed` and falls with `gravity`. Without a surface, particles are
/// emitted from the node's origin along +Y. The surface isn't drawn; wire it into a `merge_3d` as
/// well to show it. The defaults scatter all the particles at once and keep them still, which
/// suits grass on a hill or rocks on a floor; a `rate`, `lifetime` and `speed` make it an emitter,
/// such as sparks or snow.
///
/// A frame's particles depend only on the seed, the params, the surface and the time, which is
/// the frame over the composition's frame rate, so any frame renders alone and the same, and a
/// `motion_blur` blurs particles by their motion. Particles are placed on the GPU: a frame's CPU
/// work doesn't grow with `count`, and each `particles_3d` counts once towards a scene's 100000
/// objects, however many particles it draws.
///
/// The surface is taken at rest: a skinned or morphed model emits from its vertices as they are
/// stored, unposed, and an `instance_3d` in it emits from all its copies. Each object of the
/// surface emits in proportion to its area in the world, so a flat plane or a flattened box still
/// emits, and an `image_plane_3d` with a `region` emits from the part it shows. Emit points
/// follow the surface where it is at the frame. A surface with no area emits nothing.
///
/// Particles are drawn as `instance_3d` copies are, sharing the object's meshes and materials,
/// with its lights and camera left out. They are drawn at once, with no order among them, so the
/// object must be opaque or masked: a blended surface, or one showing a raster such as an image
/// plane, fails the render. Fade particles with `scale_over_life` rather than opacity. A
/// `particles_3d` may sit under an `instance_3d` of `count` 1, which places it as a group would,
/// but more copies of particles, particles of particles, and particles emitted from particles
/// fail the render.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "particles_3d",
    name = "Particles 3D",
    category = "scene",
    output = "scene"
)]
pub struct Particles3d {
    pub id: NodeId,
    #[params]
    pub params: Particles3dParamsDelegate,

    #[input(scene, optional)]
    pub surface: Option<NodeId>,
    #[input(scene)]
    pub object: Option<NodeId>,
}

impl Particles3d {
    /// Whether the particles stay where they are on every frame: they are all born at once, live
    /// forever and don't move, as the literal values of the params say.
    pub(crate) fn is_still(&self) -> bool {
        let zero = |value: &Deferred<f64>| matches!(value, Deferred::Value(value) if *value == 0.0);
        let params = &self.params;
        zero(&params.rate)
            && zero(&params.lifetime)
            && zero(&params.speed)
            && matches!(params.gravity, Deferred::Value(gravity) if gravity == [0.0; 3])
    }
}
