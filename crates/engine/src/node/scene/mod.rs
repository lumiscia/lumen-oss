//! 3D scene nodes. All but `render_3d` output a scene rather than a raster: objects, lights and
//! cameras that `render_3d` draws together, so they share depth and lighting. World units are
//! metres, Y is up and cameras look down -Z.
//!
//! The nodes and their params are always defined; what renders them needs the `3d` feature.

#[cfg(feature = "3d")]
pub(crate) mod collect;
#[cfg(feature = "3d")]
mod environment;
mod instance;
#[cfg(feature = "3d")]
pub(crate) mod mips;
#[cfg(feature = "3d")]
pub(crate) mod render;
mod text_3d;
#[cfg(feature = "3d")]
mod text_mesh;

#[cfg(feature = "3d")]
pub use collect::check_model;
#[cfg(feature = "3d")]
pub use environment::check_environment;
pub use text_3d::{Text3d, Text3dParams, Text3dParamsDelegate};

pub use instance::{Instance3d, Instance3dParams, InstanceCount, MAX_INSTANCES};

use crate::node::vector::paint::Paint;
use crate::node::{Deferred, NodeId, NodeParams};

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
    /// Base colour. An image plane multiplies its raster by it. Must be a solid color; a gradient
    /// fails `check`.
    #[meta(role = "color", solid)]
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
    /// Blocks light from lights that cast shadows. A blended surface blocks it where its alpha is
    /// at least half. Unlit and glowing surfaces block it too, so a shape standing in for a light,
    /// such as a bulb around a point light, needs this off.
    #[meta()]
    pub cast_shadows: bool,
    /// Darkens where shadows fall on it. An unlit surface shows none.
    #[meta()]
    pub receive_shadows: bool,
    /// Shows nothing but the shadows falling on it, as transparent black that darkens what the
    /// render is composited over by as much light as the shadows take away, times the colour's
    /// alpha, the opacity and, on an image plane, its raster's alpha, whatever `unlit` says. It
    /// shows none of its colours, hides what is behind it wherever that alpha isn't 0, and casts
    /// no shadows, so a ground plane catching a model's shadow can stand in for the floor of 2D
    /// footage behind it.
    #[meta()]
    pub shadow_catcher: bool,
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
            cast_shadows: true,
            receive_shadows: true,
            shadow_catcher: false,
        }
    }
}

impl Material3d {
    /// A material that shows its base colour as is, from either side.
    fn flat() -> Self {
        Self {
            unlit: true,
            double_sided: true,
            ..Self::default()
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

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ImagePlane3dParams {
    /// Width in world units, 1 being as wide as a default `shape_3d` box; the height follows the
    /// aspect ratio of the source's full display size.
    #[meta(min = 0, step = 0.1)]
    pub width: f64,
    /// Draws the plane with the opaque objects, writing depth, rather than blended. For a source
    /// with no transparent pixels, this orders the plane correctly against any geometry it
    /// intersects or crosses in depth. Alpha is then ignored: the colour's alpha, `opacity` and the
    /// source's alpha. A source pixel that is transparent shows its colour at full opacity, which
    /// is black for a fully transparent one, and a half-transparent one shows its colour
    /// undimmed. It casts a shadow over its whole rectangle. A shadow catcher is always drawn as
    /// one, so this has no effect on it.
    #[meta()]
    pub opaque: bool,
    #[meta(flatten)]
    pub placement: Placement3d,
    #[meta(flatten)]
    pub material: Material3d,
}

impl Default for ImagePlane3dParams {
    fn default() -> Self {
        Self {
            width: 1.0,
            opaque: false,
            placement: Placement3d::default(),
            material: Material3d::flat(),
        }
    }
}

/// Shows a raster, such as text or video, on a flat rectangle facing +Z. Unlit and double-sided by
/// default, so it looks as it does in 2D while it faces the camera. A plane is drawn blended,
/// ordered by its centre among blended objects, so its transparent parts show what is behind it.
/// Planes that intersect, or overlap while crossing in depth, can show in the wrong order; layers
/// separated in depth, and opaque shapes in front of or behind planes, are ordered correctly.
/// `opaque` draws a plane that has no transparent pixels with the opaque objects instead.
/// A source keeps its detail when it is shown at its own size or larger; shown smaller, it is
/// filtered with mipmaps, trilinearly and anisotropically, before `render_3d`'s `supersample`
/// averages it.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "image_plane_3d",
    name = "Image Plane 3D",
    category = "scene",
    output = "scene"
)]
pub struct ImagePlane3d {
    pub id: NodeId,
    #[params]
    pub params: ImagePlane3dParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Model3dParams {
    /// A binary glTF (`.glb`) file with its buffers and images inside it: a path or `media`
    /// alias, or `asset:<id>` for a model asset of the host's.
    #[meta(role = "source_id")]
    #[media(model)]
    pub source: String,
    /// The animation clip to play, by its name or else its index; empty plays none. A model's
    /// clips are listed in the `animations` of its upload's `metadata` on a host that takes
    /// uploads, and by `modelInfo` in the browser bindings.
    #[meta()]
    pub animation: String,
    /// Playback speed multiplier.
    #[meta(step = 0.1)]
    pub animation_speed: f64,
    /// Seconds into the clip at the clip's first frame.
    #[meta(step = 0.1)]
    pub animation_offset: f64,
    /// Plays the clip again from its start once it ends, rather than holding its last pose.
    #[meta()]
    pub animation_loop: bool,
    /// The material, by name, whose surfaces show the `texture` input instead, unlit. A model's
    /// materials are listed in the `materials` of its upload's `metadata` on a host that takes
    /// uploads, and by `modelInfo` in the browser bindings.
    #[meta()]
    pub texture_material: String,
    /// Blocks light from lights that cast shadows, in the posed shape and where masked or blended
    /// materials' alpha reaches their cutoff, or half for blended ones. Unlit and glowing
    /// materials block it too, so a model of a lamp around a light needs this off.
    #[meta()]
    pub cast_shadows: bool,
    /// Darkens where shadows fall on its lit surfaces.
    #[meta()]
    pub receive_shadows: bool,
    /// Adds the model's own `KHR_lights_punctual` lights to the scene, where its nodes place
    /// them, as `light_3d` nodes there would light it: a directional light's intensity, in lux, and
    /// a point or spot light's, in candela, are `light_3d` intensities as they are, so a point
    /// light of intensity `I` lights a surface `d` metres away as a directional one of `I / d²`
    /// does. A spot light's cone fades from its inner angle out to its outer one. They cast no
    /// shadows, and a scene they light gets no default lights.
    #[meta()]
    pub lights: bool,
    /// The model's camera to view the scene from, by its name or else its index, placed by the
    /// first node that places it, without that node's scale; empty uses none. It counts as a
    /// `camera_3d` at this node, so the first camera in the scene still wins. Its aspect ratio is
    /// the output's. A model's cameras are listed in the `cameras` of its upload's `metadata` on
    /// a host that takes uploads, and by `modelInfo` in the browser bindings.
    #[meta()]
    pub camera: String,
    #[meta(flatten)]
    pub placement: Placement3d,
}

impl Default for Model3dParams {
    fn default() -> Self {
        Self {
            source: String::new(),
            animation: String::new(),
            animation_speed: 1.0,
            animation_offset: 0.0,
            animation_loop: true,
            texture_material: String::new(),
            cast_shadows: true,
            receive_shadows: true,
            lights: false,
            camera: String::new(),
            placement: Placement3d::default(),
        }
    }
}

/// A glTF model: the default scene of a `.glb`, with its metallic-roughness materials, vertex
/// colours, two sets of texture coordinates with `KHR_texture_transform`, skins, morph targets
/// and animation clips. A raster wired into `texture` shows on the surfaces of the material
/// `texture_material` names, such as a screen. The model's own lights and a camera are added with
/// `lights` and `camera`. Points, lines, and triangle strips and fans aren't drawn, with a
/// `scene.model_primitives_skipped` warning.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "model_3d",
    name = "Model 3D",
    category = "scene",
    output = "scene"
)]
pub struct Model3d {
    pub id: NodeId,
    #[params]
    pub params: Model3dParamsDelegate,

    #[input(optional)]
    pub texture: Option<NodeId>,
}

impl Model3d {
    /// Whether the model may pose differently from frame to frame: it plays a clip, at a speed
    /// other than 0.
    pub(crate) fn animates(&self) -> bool {
        let plays = match &self.params.animation {
            Deferred::Value(clip) => !clip.is_empty(),
            Deferred::Expr(_) => true,
        };
        plays && !matches!(self.params.animation_speed, Deferred::Value(speed) if speed == 0.0)
    }
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
    /// Light colour. Must be a solid color; a gradient fails `check`.
    #[meta(role = "color", solid)]
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
    /// Casts shadows from objects that block it onto those behind them. A scene draws shadows for
    /// its first 8 lights that cast them, in the order its scene inputs are wired, and warns
    /// about the rest. A spot light's shadows reach 170° across at most.
    #[meta()]
    pub cast_shadows: bool,
    /// Width in world units over which shadow edges blur, up to 16 texels of the light's shadow
    /// map, which is up to 4096 texels across and shared among the shadow-casting lights.
    #[meta(min = 0, step = 0.01)]
    pub shadow_softness: f64,
    /// How far from the camera, along its view, a directional light's shadows reach, fading out
    /// over the last tenth. Its shadow map spreads over what the camera sees of the scene up to
    /// here, so a larger distance trades sharpness for reach; a scene the camera sees nearer
    /// keeps its full sharpness.
    #[meta(min = 0.01, step = 1)]
    pub shadow_distance: f64,
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
            cast_shadows: false,
            shadow_softness: 0.02,
            shadow_distance: 50.0,
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

/// What `render_3d` draws into its raster. Depth and normals hold data, with no transfer function
/// applied, drawn from opaque surfaces and shadow catchers, masked surfaces where they reach their
/// cutoff, and blended ones, such as image planes, where their alpha reaches half. They skip
/// lighting, shadows and the environment, and ignore `supersample`: each pixel holds the nearest
/// surface at its centre, never an average across an edge.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum Render3dOutput {
    /// The lit scene.
    #[default]
    Color = 0,
    /// Distance from the camera along its view, from 0 at `depth_near` to 1 at `depth_far`, as
    /// opaque grey; 1 where nothing is drawn.
    Depth = 1,
    /// The surface's normal as it is shaded, in the camera's space with +X right, +Y up and +Z
    /// towards the camera, each of its components `n` stored as `n * 0.5 + 0.5` in red, green and
    /// blue; transparent where nothing is drawn.
    Normal = 2,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Render3dParams {
    /// What the raster holds: the lit scene, its depth or its normals. A composition that needs
    /// more than one draws the same scene with a `render_3d` for each.
    #[meta(kind = "enum", enum_type = Render3dOutput, constant)]
    pub output: Render3dOutput,
    /// Distance from the camera, in world units, that `depth` output maps to 0. 0 uses the
    /// camera's `near`.
    #[meta(min = 0, step = 0.1)]
    pub depth_near: f64,
    /// Distance from the camera, in world units, that `depth` output maps to 1, and past which it
    /// stays 1. 0 uses the camera's `far`, which spreads the 0 to 1 range thin over a scene much
    /// nearer than it.
    #[meta(min = 0, step = 1)]
    pub depth_far: f64,
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
    /// Brightness of the light from all around that surfaces reflect and are lit by: the
    /// `environment_map` when one is set, and otherwise a built-in studio of softboxes. 0 turns it
    /// off.
    #[meta(min = 0, step = 0.1)]
    pub environment: f64,
    /// An equirectangular Radiance `.hdr` or OpenEXR `.exr` file that lights the scene from all
    /// around in place of the studio: a path or `media` alias, or `asset:<id>` for an environment
    /// map asset of the host's. Its centre is the view along -Z, with +X to its right and +Y at
    /// its top, and it is stretched over the whole sphere whatever its aspect. Non-finite and
    /// negative pixels are read as black. While a map lights a scene without lights, it gets no
    /// default ones. Empty uses the studio.
    #[meta(role = "source_id")]
    #[media(environment)]
    pub environment_map: String,
    /// Turn of the environment map about +Y in degrees, as a yaw turns an object.
    #[meta(step = 1)]
    pub environment_rotation: f64,
    /// Shows the environment map behind the scene, through the exposure and tone mapping but not
    /// the fog, which makes the output opaque. Shadow catchers darken it.
    #[meta()]
    pub environment_background: bool,
    /// How blurred the background is, as a surface of this roughness reflects it: 0 is sharp.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub environment_background_blur: f64,
    /// Colour surfaces fade to with distance. Must be a solid color; a gradient fails `check`.
    #[meta(role = "color", solid)]
    pub fog_color: Paint,
    /// How quickly surfaces fade into the fog: the fraction of their colour left after `d` world
    /// units is e^(-density * d). 0 turns fog off.
    #[meta(min = 0, step = 0.01)]
    pub fog_density: f64,
}

impl Default for Render3dParams {
    fn default() -> Self {
        Self {
            output: Render3dOutput::Color,
            depth_near: 0.0,
            depth_far: 0.0,
            width: 0,
            height: 0,
            supersample: 2,
            exposure: 0.0,
            tone_mapping: ToneMapping::None,
            environment: 1.0,
            environment_map: String::new(),
            environment_rotation: 0.0,
            environment_background: false,
            environment_background_blur: 0.0,
            fog_color: Paint::solid([255, 255, 255, 255]),
            fog_density: 0.0,
        }
    }
}

/// Draws a 3D scene into a raster, with a transparent background unless it shows its environment
/// map; or, by `output`, its depth or normals, for 2D effects such as fog.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "render_3d", name = "Render 3D", category = "scene")]
pub struct Render3d {
    pub id: NodeId,
    #[params]
    pub params: Render3dParamsDelegate,

    #[input(scene)]
    pub scene: Option<NodeId>,
}
