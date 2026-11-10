//! 3D scene nodes. All but `render_3d` output a scene rather than a raster: objects, lights and
//! cameras that `render_3d` draws together, so they share depth and lighting. World units are
//! metres, Y is up and cameras look down -Z.
//!
//! The nodes and their params are always defined; what renders them needs the `3d` feature.

#[cfg(feature = "3d")]
pub(crate) mod collect;
#[cfg(feature = "3d")]
pub(crate) mod render;

use crate::node::vector::paint::Paint;
use crate::node::{NodeId, NodeParams};

/// Where something sits and which way it faces.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Pose3d {
    /// Position in world units along X, Y and Z, in the parent's space.
    #[meta(step = 0.1)]
    pub position: [f64; 3],
    /// Rotation in degrees about X, Y and Z, applied as yaw about Y, then pitch about the turned
    /// X, then roll about the turned Z.
    #[meta(step = 1)]
    pub rotation: [f64; 3],
}

impl Default for Pose3d {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0; 3],
        }
    }
}

/// Where an object sits in its parent's space.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Placement3d {
    #[meta(flatten)]
    pub pose: Pose3d,
    /// Scale multiplier along X, Y and Z, applied before the rotation.
    #[meta(step = 0.1)]
    pub scale: [f64; 3],
}

impl Default for Placement3d {
    fn default() -> Self {
        Self {
            pose: Pose3d::default(),
            scale: [1.0; 3],
        }
    }
}

/// How a surface responds to light.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Material3d {
    /// Base colour. Must be a solid color.
    #[meta(role = "color")]
    pub color: Paint,
    /// 0 is a dielectric such as plastic, 1 a bare metal.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub metallic: f64,
    /// 0 is a mirror finish, 1 fully diffuse.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub roughness: f64,
    /// Light the surface gives off on its own, as a multiple of its base colour.
    #[meta(min = 0, step = 0.1)]
    pub emissive: f64,
    /// Opacity multiplier. Anything below 1 is drawn blended, after opaque objects.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
    /// Shows the base colour as is, ignoring lights.
    #[meta()]
    pub unlit: bool,
    /// Draws the back of each face too.
    #[meta()]
    pub double_sided: bool,
}

impl Default for Material3d {
    fn default() -> Self {
        Self {
            color: Paint::solid([255, 255, 255, 255]),
            metallic: 0.0,
            roughness: 0.5,
            emissive: 0.0,
            opacity: 1.0,
            unlit: false,
            double_sided: false,
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum Shape3dKind {
    #[default]
    Box = 0,
    Sphere = 1,
    /// A flat rectangle facing +Y.
    Plane = 2,
    Cylinder = 3,
    /// A ring whose tube is a quarter of `size` thick along Y.
    Torus = 4,
    Cone = 5,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Shape3dParams {
    /// Primitive to build.
    #[meta(kind = "enum", enum_type = Shape3dKind)]
    pub shape: Shape3dKind,
    /// Width, height and depth in world units, along X, Y and Z before the rotation.
    #[meta(step = 0.1)]
    pub size: [f64; 3],
    #[meta(flatten)]
    pub placement: Placement3d,
    #[meta(flatten)]
    pub material: Material3d,
}

impl Default for Shape3dParams {
    fn default() -> Self {
        Self {
            shape: Shape3dKind::Box,
            size: [1.0; 3],
            placement: Placement3d::default(),
            material: Material3d::default(),
        }
    }
}

/// A primitive solid: box, sphere, plane, cylinder, torus or cone, centred on its position.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "shape_3d",
    name = "Shape 3D",
    category = "scene",
    output = "scene"
)]
pub struct Shape3d {
    pub id: NodeId,
    #[params]
    pub params: Shape3dParamsDelegate,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum Projection3d {
    /// Farther things look smaller.
    #[default]
    Perspective = 0,
    /// Things keep their size at any distance, with parallel lines staying parallel.
    Orthographic = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Camera3dParams {
    /// How the scene is projected onto the image.
    #[meta(kind = "enum", enum_type = Projection3d)]
    pub projection: Projection3d,
    /// Vertical field of view in degrees, with a perspective projection.
    #[meta(min = 1, max = 179, step = 1)]
    pub fov: f64,
    /// Height of the view in world units, with an orthographic projection.
    #[meta(min = 0, step = 0.1)]
    pub ortho_height: f64,
    /// Nearest distance drawn, in world units.
    #[meta(min = 0, step = 0.01)]
    pub near: f64,
    /// Farthest distance drawn, in world units.
    #[meta(min = 0, step = 1)]
    pub far: f64,
    /// Aims the camera at `target`, ignoring its rotation.
    #[meta()]
    pub look_at: bool,
    /// Point along X, Y and Z the camera aims at while `look_at` is on, in its parent's space.
    #[meta(step = 0.1)]
    pub target: [f64; 3],
    #[meta(flatten)]
    pub pose: Pose3d,
}

impl Default for Camera3dParams {
    fn default() -> Self {
        Self {
            projection: Projection3d::Perspective,
            fov: 40.0,
            ortho_height: 4.0,
            near: 0.05,
            far: 1000.0,
            look_at: false,
            target: [0.0; 3],
            pose: Pose3d {
                position: [0.0, 0.0, 5.0],
                ..Pose3d::default()
            },
        }
    }
}

/// The viewpoint `render_3d` draws from, looking down its -Z. The first camera in a scene wins;
/// without one, `render_3d` views from 5 units along +Z with a 40° field of view.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "camera_3d",
    name = "Camera 3D",
    category = "scene",
    output = "scene"
)]
pub struct Camera3d {
    pub id: NodeId,
    #[params]
    pub params: Camera3dParamsDelegate,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum Light3dKind {
    /// Parallel rays along the light's -Z, like the sun.
    #[default]
    Directional = 0,
    /// Shines every way from its position.
    Point = 1,
    /// A cone along the light's -Z from its position.
    Spot = 2,
    /// Lights every surface evenly from all around.
    Ambient = 3,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Light3dParams {
    /// Kind of light.
    #[meta(kind = "enum", enum_type = Light3dKind)]
    pub light: Light3dKind,
    /// Light colour. Must be a solid color.
    #[meta(role = "color")]
    pub color: Paint,
    /// Unitless brightness multiplier; point and spot lights fall off with the square of distance.
    #[meta(min = 0, step = 0.1)]
    pub intensity: f64,
    /// Distance past which a point or spot light has no effect; 0 is unlimited.
    #[meta(min = 0, step = 0.1)]
    pub range: f64,
    /// Full angle of a spot light's cone in degrees.
    #[meta(min = 0, max = 179, step = 1)]
    pub spot_angle: f64,
    /// Fraction of a spot light's cone, from its edge, that fades out.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub spot_softness: f64,
    #[meta(flatten)]
    pub pose: Pose3d,
}

impl Default for Light3dParams {
    fn default() -> Self {
        Self {
            light: Light3dKind::Directional,
            color: Paint::solid([255, 255, 255, 255]),
            intensity: 3.0,
            range: 0.0,
            spot_angle: 45.0,
            spot_softness: 0.2,
            pose: Pose3d {
                rotation: [-45.0, 30.0, 0.0],
                ..Pose3d::default()
            },
        }
    }
}

/// A light. A scene without any is lit by a default key light and ambient fill.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "light_3d",
    name = "Light 3D",
    category = "scene",
    output = "scene"
)]
pub struct Light3d {
    pub id: NodeId,
    #[params]
    pub params: Light3dParamsDelegate,
}

#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct Merge3dParams {
    #[meta(flatten)]
    pub placement: Placement3d,
}

/// Groups scenes into one, placed together by its transform.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "merge_3d",
    name = "Merge 3D",
    category = "scene",
    output = "scene"
)]
pub struct Merge3d {
    pub id: NodeId,
    #[params]
    pub params: Merge3dParamsDelegate,

    #[input(scene, optional, variadic)]
    pub objects: Vec<NodeId>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ToneMapping {
    /// Clips light above 1.
    #[default]
    None = 0,
    /// The ACES filmic curve.
    Aces = 1,
    /// AgX, which desaturates bright light more naturally than ACES.
    Agx = 2,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Render3dParams {
    /// Output width in pixels. Use 0 to match the composition width.
    #[meta(min = 0, enforced_min, step = 1, constant)]
    pub width: u32,
    /// Output height in pixels. Use 0 to match the composition height.
    #[meta(min = 0, enforced_min, step = 1, constant)]
    pub height: u32,
    /// Renders this many times wider and taller, then averages down, to antialias edges.
    #[meta(min = 1, max = 4, enforced_min, step = 1)]
    pub supersample: u32,
    /// Exposure adjustment in stops before tone mapping.
    #[meta(step = 0.1)]
    pub exposure: f64,
    /// How light brighter than white is brought into range.
    #[meta(kind = "enum", enum_type = ToneMapping)]
    pub tone_mapping: ToneMapping,
    /// Brightness of the built-in studio of softboxes that surfaces reflect and are lit by from
    /// all around. 0 turns it off.
    #[meta(min = 0, step = 0.1)]
    pub environment: f64,
    /// Colour surfaces fade to with distance. Must be a solid color.
    #[meta(role = "color")]
    pub fog_color: Paint,
    /// How quickly surfaces fade into the fog: the fraction of their colour left after `d` world
    /// units is e^(-density * d). 0 turns fog off.
    #[meta(min = 0, step = 0.01)]
    pub fog_density: f64,
}

impl Default for Render3dParams {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            supersample: 2,
            exposure: 0.0,
            tone_mapping: ToneMapping::None,
            environment: 1.0,
            fog_color: Paint::solid([255, 255, 255, 255]),
            fog_density: 0.0,
        }
    }
}

/// Draws a 3D scene into a raster, with a transparent background.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "render_3d", name = "Render 3D", category = "scene")]
pub struct Render3d {
    pub id: NodeId,
    #[params]
    pub params: Render3dParamsDelegate,

    #[input(scene)]
    pub scene: Option<NodeId>,
}
