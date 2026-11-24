//! 3D scene nodes. All but `render_3d` output a scene rather than a raster: objects, lights and
//! cameras that `render_3d` draws together, so they share depth and lighting. World units are
//! metres, Y is up and cameras look down -Z.
//!
//! The nodes and their params are always defined; what renders them needs the `3d` feature.

#[cfg(feature = "3d")]
pub(crate) mod collect;
#[cfg(feature = "3d")]
mod environment;
mod extrude_3d;
mod instance;
mod instance_copies;
mod lathe_3d;
mod matte;
#[cfg(feature = "3d")]
pub(crate) mod mips;
mod particles;
mod record;
mod region;
#[cfg(feature = "3d")]
pub(crate) mod render;
#[cfg(feature = "3d")]
mod solid_mesh;
mod text_3d;
#[cfg(feature = "3d")]
mod text_mesh;

#[cfg(feature = "3d")]
pub use collect::check_model;
#[cfg(feature = "3d")]
pub use environment::check_environment;
pub use extrude_3d::{Extrude3d, Extrude3dParams, Extrude3dParamsDelegate};
pub use lathe_3d::{Lathe3d, Lathe3dParams, Lathe3dParamsDelegate};
pub(crate) use matte::MATTE_NAMES;
pub use matte::{MAX_MATTES, MatteNames};
pub use text_3d::{Text3d, Text3dParams, Text3dParamsDelegate};

pub use instance::{Instance3d, Instance3dParams, InstanceCount, MAX_INSTANCES};
pub use instance_copies::{InstanceCopies, InstanceCopy};
pub use particles::{MAX_PARTICLES, ParticleCount, Particles3d, Particles3dParams};
pub(crate) use region::REGION;
pub use region::{Region, RegionDelegate};

use crate::node::processing::RasterSampling;
use crate::node::vector::paint::Paint;
use crate::node::{Deferred, NodeId, NodeParams, OutputRef};

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

/// How a surface's alpha decides what it shows.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum MaterialAlphaMode {
    /// Draws the surface blended where its alpha is below 1, after the opaque objects.
    #[default]
    Blend = 0,
    /// Draws the surface with the opaque objects, writing depth, showing the texels whose alpha
    /// reaches `alpha_cutoff` at full opacity and discarding the rest.
    Mask = 1,
}

/// How a surface responds to light.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Material3dParams {
    /// Base colour, which multiplies the `texture` on a shape and the raster on an image plane.
    /// Must be a solid color; a gradient fails `check`.
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
    /// Opacity multiplier. With `alpha_mode` `blend`, anything below 1 is drawn blended, after
    /// opaque objects.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
    /// `blend` draws the surface blended where its alpha is below 1. `mask` draws it with the
    /// opaque objects, so it writes depth and crossing masked surfaces order correctly: texels
    /// whose alpha, the colour's times `opacity` times the texture's or raster's, is below
    /// `alpha_cutoff` are discarded and the rest show at full opacity. Masked surfaces cast
    /// shadows shaped by the cut-out. It takes precedence over `opaque` on an `image_plane_3d`,
    /// and with `shadow_catcher` fails `check`.
    #[meta()]
    pub alpha_mode: MaterialAlphaMode,
    /// The alpha a texel needs to show when `alpha_mode` is `mask`. Not read otherwise.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub alpha_cutoff: f64,
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
    /// Darkens where shadows fall on it. An unlit surface shows them as `unlit_shadows` says.
    #[meta()]
    pub receive_shadows: bool,
    /// How much an unlit surface darkens where shadows fall on it, from 0, which shows none, to
    /// 1, where a fully shadowed point is black. Lit surfaces ignore it, as do surfaces that
    /// don't receive shadows.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub unlit_shadows: f64,
    /// Shows nothing but the shadows falling on it, as transparent black that darkens what the
    /// render is composited over by as much light as the shadows take away, times the colour's
    /// alpha, the opacity and, on an image plane, its raster's alpha, whatever `unlit` says. It
    /// shows none of its colours, hides what is behind it wherever that alpha isn't 0, and casts
    /// no shadows, so a ground plane catching a model's shadow can stand in for the floor of 2D
    /// footage behind it.
    #[meta()]
    pub shadow_catcher: bool,
    /// How a shape reads its `texture`, as `sampling` on `image_plane_3d`: `nearest` gives each
    /// pixel the one texel under it at any size, for pixel art.
    #[meta()]
    pub sampling: RasterSampling,
    /// With a `projector` wired, surfaces the projector can't see, behind surfaces that cast
    /// shadows, show the colour alone.
    #[meta()]
    pub projector_occlusion: bool,
}

impl Default for Material3dParams {
    fn default() -> Self {
        Self {
            color: Paint::solid([255, 255, 255, 255]),
            metallic: 0.0,
            roughness: 0.5,
            emissive: 0.0,
            opacity: 1.0,
            alpha_mode: MaterialAlphaMode::Blend,
            alpha_cutoff: 0.5,
            unlit: false,
            double_sided: false,
            cast_shadows: true,
            receive_shadows: true,
            unlit_shadows: 0.0,
            shadow_catcher: false,
            sampling: RasterSampling::Linear,
            projector_occlusion: true,
        }
    }
}

impl Material3dParams {
    /// A material that shows its base colour as is, from either side.
    #[cfg(feature = "3d")]
    fn flat() -> Self {
        Self {
            unlit: true,
            double_sided: true,
            ..Self::default()
        }
    }
}

/// How a surface responds to light, for the 3D objects it is wired into through their `material`
/// input. One `material_3d` can be wired to any number of objects, which then share its params,
/// so changing it changes them all. Each param takes an expression as it does on any node.
/// Objects with no material wired in use a lit, single-sided white one; an `image_plane_3d` uses
/// an unlit, double-sided one. A raster wired into `texture` shows on a `shape_3d`, multiplied by
/// `color`, through the texture coordinates `shape_3d` describes, lit and shadowed like the rest
/// of the surface, and filtered as an image plane's source is unless `sampling` is `nearest`.
/// Its alpha counts where the material blends or masks, through `opacity` or the colour's alpha.
/// Wired into any other object, a material with a texture fails the render, unless it has a
/// `projector`; an `image_plane_3d` shows its own source and fails with either.
///
/// A scene wired into `projector` projects the `texture` from its camera, chosen as `render_3d`
/// would choose it, in place of texture coordinates, so it shows on any object but an image
/// plane, `instance_3d` copies included. Each point of the surface takes the texel the projector
/// sees it at: the texture fills the projector's view at the texture's own aspect ratio, with the
/// camera's `fov` or `ortho_height`, `near` and `far`. Faces turned away from the projector,
/// points outside its view and, with `projector_occlusion`, points it can't see show `color`
/// alone. The projector stands where its own scene places it, whatever places the objects, at the
/// frame being drawn, so a moving camera moves the projection: a plate projected onto geometry
/// wants a still `camera_3d` of its own, apart from the camera the scene is drawn from. A
/// `projector` without a `texture` fails the render.
///
/// `alpha_mode` `mask` draws a cut-out card, such as a `text` raster on an `image_plane_3d` whose
/// material is unlit and double-sided, with the opaque objects so it orders correctly where cards
/// cross. As a glTF `MASK` material does, it shows only the texels that reach `alpha_cutoff`.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "material_3d",
    name = "Material 3D",
    category = "scene",
    output = "material"
)]
pub struct Material3d {
    pub id: NodeId,
    #[params]
    pub params: Material3dParamsDelegate,

    #[input(raster, optional)]
    pub texture: Option<OutputRef>,
    /// A scene whose camera projects `texture` onto the surfaces, usually a `camera_3d` alone.
    #[input(scene, optional)]
    pub projector: Option<OutputRef>,
}

impl Material3d {
    /// What is wrong with a node whose JSON `params` set `alpha_mode` to `mask` and
    /// `shadow_catcher` to true, if they do.
    pub(crate) fn mask_conflict(
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<String> {
        (params.get("alpha_mode").and_then(|mode| mode.as_str()) == Some("mask")
            && params.get("shadow_catcher") == Some(&serde_json::Value::Bool(true)))
        .then(|| MASKED_CATCHER.to_string())
    }
}

/// Why a shadow catcher can't be masked.
const MASKED_CATCHER: &str = "a shadow catcher's alpha sets its coverage; it can't be masked";

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
    #[meta()]
    pub shape: Shape3dKind,
    /// Width, height and depth in world units, along X, Y and Z before the rotation.
    #[meta(step = 0.1)]
    pub size: [f64; 3],
    /// The matte its visible coverage goes to, by a name `render_3d`'s `mattes` lists, for that
    /// render's `matte` output. Empty takes the matte of the nearest `merge_3d`, `instance_3d` or
    /// `particles_3d` holding it that names one, or none.
    #[meta()]
    pub matte: String,
    #[meta(flatten)]
    pub placement: Placement3d,
}

impl Default for Shape3dParams {
    fn default() -> Self {
        Self {
            shape: Shape3dKind::Box,
            size: [1.0; 3],
            matte: String::new(),
            placement: Placement3d::default(),
        }
    }
}

/// A primitive solid: box, sphere, plane, cylinder, torus or cone, centred on its position. Lit,
/// single-sided and white unless a `material_3d` is wired into `material`. Its material's
/// `texture` shows upright seen from outside: whole on each face of a box, the top and bottom
/// faces upright to a camera on +Z tilted to look at them, and whole on a plane seen from above,
/// its top edge towards -Z. A sphere, cylinder, cone or torus wraps it around Y, its left edge at
/// -Z and running towards -X, so a camera on +Z sees its middle. A sphere maps it
/// equirectangularly from the top pole to the bottom one, and a cylinder's or cone's side from
/// top to bottom, with the circle inscribed in the texture on each cap, upright as on a box. A
/// torus wraps the texture's height around its tube, from the inner equator over the top, so its
/// middle row runs around the outer equator.
///
/// A box takes a material for each face from `top_material`, `bottom_material`, `left_material`,
/// `right_material`, `front_material` and `back_material`, which are its +Y, -Y, -X, +X, +Z and
/// -Z faces in its own axes before `rotation`; the front faces a camera looking down -Z. A face
/// with none wired uses `material`, and each face shows its own material's texture. A face
/// material wired into any other shape fails the render naming the input. The faces draw as one
/// object each, merged where neighbours share a material, from the same mesh, so a box with face
/// materials costs a draw for each run of faces, and under `instance_3d` each draws all its
/// copies.
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

    #[input(material, optional)]
    pub material: Option<OutputRef>,
    #[input(material, optional)]
    pub top_material: Option<OutputRef>,
    #[input(material, optional)]
    pub bottom_material: Option<OutputRef>,
    #[input(material, optional)]
    pub left_material: Option<OutputRef>,
    #[input(material, optional)]
    pub right_material: Option<OutputRef>,
    #[input(material, optional)]
    pub front_material: Option<OutputRef>,
    #[input(material, optional)]
    pub back_material: Option<OutputRef>,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ImagePlane3dParams {
    /// Width in world units, 1 being as wide as a default `shape_3d` box; the height follows the
    /// aspect ratio of the source's full display size.
    #[meta(min = 0, step = 0.1)]
    pub width: f64,
    /// Shows only this part of the source, `[x, y, width, height]` in the source's pixels from its
    /// top left, on a plane that covers just that part, so a plane showing a region is where it
    /// would be and as big as it would be with the whole source on it: it looks as a plane
    /// showing a `crop` of that region does, where `width` is still the width of the whole
    /// source, and `position`, `rotation` and `scale` pivot about the whole source's centre.
    /// Cut off at the source's edges as `crop` cuts its region, and a region with no pixel
    /// left shows nothing. Each of the four numbers takes an integer or an expression, which is
    /// truncated toward zero as `crop`'s are; the whole region cannot be an expression. Unset
    /// shows the whole source. Planes showing parts of one source share its texture and mip
    /// levels and draw together, so a region costs nothing beyond the plane, where a `crop`
    /// costs a raster the size of the source and a draw of its own. Sampling is kept inside the
    /// region, so what is outside it isn't meant to show. To keep it so under strong minification,
    /// the band along each edge of the region that is inside the source, and all of a narrow
    /// region, is sampled from a sharper mip level than its footprint, since a texel of a shared
    /// level can hold pixels from both sides of the edge: it can alias or shimmer there where a
    /// `crop` would be smooth, across the whole of a piece drawn much smaller than its pixels.
    #[meta()]
    pub region: Region,
    /// Draws the plane with the opaque objects, writing depth, rather than blended. For a source
    /// with no transparent pixels, this orders the plane correctly against any geometry it
    /// intersects or crosses in depth. Alpha is then ignored: the colour's alpha, `opacity` and the
    /// source's alpha. A source pixel that is transparent shows its colour at full opacity, which
    /// is black for a fully transparent one, and a half-transparent one shows its colour
    /// undimmed. It casts a shadow over its whole rectangle. A shadow catcher is always drawn as
    /// one, so this has no effect on it.
    #[meta()]
    pub opaque: bool,
    /// `nearest` gives each pixel the one source texel under it at any size, for pixel art:
    /// magnified, texels stay hard-edged squares; shown smaller, a pixel picks one texel instead
    /// of averaging the ones it covers, so it aliases and shimmers where `linear` is smooth.
    /// `render_3d`'s `supersample` still averages its samples within each pixel.
    #[meta()]
    pub sampling: RasterSampling,
    /// The matte its visible coverage goes to, by a name `render_3d`'s `mattes` lists, for that
    /// render's `matte` output. Empty takes the matte of the nearest `merge_3d`, `instance_3d` or
    /// `particles_3d` holding it that names one, or none.
    #[meta()]
    pub matte: String,
    #[meta(flatten)]
    pub placement: Placement3d,
}

impl Default for ImagePlane3dParams {
    fn default() -> Self {
        Self {
            width: 1.0,
            region: Region::default(),
            opaque: false,
            sampling: RasterSampling::Linear,
            matte: String::new(),
            placement: Placement3d::default(),
        }
    }
}

/// Shows a raster, such as text or video, on a flat rectangle facing +Z. With no `material_3d`
/// wired into `material` it is unlit and double-sided, so it looks as it does in 2D while it faces
/// the camera. A `material_3d` applies exactly as it is set, so a default one makes the plane lit
/// and single-sided; set its `unlit` and `double_sided` to keep the plane as it looks unwired.
/// A plane is drawn blended, ordered by its centre among blended objects, so its transparent parts
/// show what is behind it. Planes that intersect, or overlap while crossing in depth, can show in
/// the wrong order; layers separated in depth, and opaque shapes in front of or behind planes, are
/// ordered correctly. `opaque` draws a plane that has no transparent pixels with the opaque
/// objects instead. A source keeps its detail when it is shown at its own size or larger; shown
/// smaller, it is filtered with mipmaps, trilinearly and anisotropically, before `render_3d`'s
/// `supersample` averages it, unless `sampling` is `nearest`.
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
    pub source: Option<OutputRef>,
    #[input(material, optional)]
    pub material: Option<OutputRef>,
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
    /// The matte its visible coverage goes to, by a name `render_3d`'s `mattes` lists, for that
    /// render's `matte` output. Empty takes the matte of the nearest `merge_3d`, `instance_3d` or
    /// `particles_3d` holding it that names one, or none.
    #[meta()]
    pub matte: String,
    /// With a `projector` wired, the surfaces showing `texture` that the projector can't see,
    /// behind surfaces that cast shadows, show white.
    #[meta()]
    pub projector_occlusion: bool,
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
            matte: String::new(),
            projector_occlusion: true,
            placement: Placement3d::default(),
        }
    }
}

/// A glTF model: the default scene of a `.glb`, with its metallic-roughness materials, vertex
/// colours, two sets of texture coordinates with `KHR_texture_transform`, skins, morph targets
/// and animation clips. A raster wired into `texture` shows on the surfaces of the material
/// `texture_material` names, such as a screen. With a scene wired into `projector` as well, the
/// texture is projected onto those surfaces from the scene's camera instead, as a `material_3d`'s
/// `projector` projects its texture, where faces turned away and points out of view show white;
/// naming the model's only material projects a plate onto all of it. The model's own lights and a
/// camera are added with `lights` and `camera`. Points, lines, and triangle strips and fans aren't
/// drawn, with a `scene.model_primitives_skipped` warning.
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
    pub texture: Option<OutputRef>,
    /// A scene whose camera projects `texture` onto the surfaces it shows on.
    #[input(scene, optional)]
    pub projector: Option<OutputRef>,
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
    #[meta()]
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
    /// Aperture of the lens as an f-number, which blurs what is nearer or farther than
    /// `focus_distance` as a camera does: lower is blurrier. The aperture is the focal length over
    /// this, for a full-frame (36 × 24 mm) camera with this `fov` and world units in metres. 0 is
    /// a pinhole, which keeps everything sharp. `render_3d`'s `dof_samples` sets how smooth the
    /// blur is. An orthographic projection ignores it.
    #[meta(min = 0, step = 0.1)]
    pub f_stop: f64,
    /// Distance from the camera along its view, in world units, that stays sharp while `f_stop`
    /// is above 0.
    #[meta(min = 0, step = 0.1)]
    pub focus_distance: f64,
    /// Blades of the aperture, which shape a blurred highlight: 0 is a circle, and 3 to 16 a
    /// polygon with that many sides.
    #[meta(min = 0, max = 16, step = 1)]
    pub aperture_blades: i64,
    /// Turn of the aperture polygon in degrees, clockwise. At 0 one corner points up.
    #[meta(step = 1)]
    pub aperture_rotation: f64,
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
            f_stop: 0.0,
            focus_distance: 5.0,
            aperture_blades: 0,
            aperture_rotation: 0.0,
            look_at: false,
            target: [0.0; 3],
            pose: Pose3d {
                position: [0.0, 0.0, 5.0],
                ..Pose3d::default()
            },
        }
    }
}

/// Most blades of a camera's aperture.
const MAX_APERTURE_BLADES: i64 = 16;

/// What is wrong with `blades` as a `camera_3d`'s `aperture_blades`, if anything.
pub(crate) fn aperture_blades_error(blades: i64) -> Option<String> {
    (blades != 0 && !(3..=MAX_APERTURE_BLADES).contains(&blades)).then(|| {
        format!(
            "`aperture_blades` is {blades}; it must be 0 for a circle or from 3 to \
             {MAX_APERTURE_BLADES}"
        )
    })
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
    #[meta()]
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

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Merge3dParams {
    /// Whether the merge and everything wired into it is in the scene. While false, nothing
    /// reached only through it is evaluated, drawn or decoded: objects, lights and cameras
    /// alike.
    #[meta()]
    pub visible: bool,
    /// The matte, by a name `render_3d`'s `mattes` lists, of every object inside it that names
    /// none of its own, for that render's `matte` output. Empty passes on the matte of the
    /// nearest `merge_3d`, `instance_3d` or `particles_3d` holding it that names one, or none.
    #[meta()]
    pub matte: String,
    #[meta(flatten)]
    pub placement: Placement3d,
}

impl Default for Merge3dParams {
    fn default() -> Self {
        Self {
            visible: true,
            matte: String::new(),
            placement: Placement3d::default(),
        }
    }
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
    pub objects: Vec<OutputRef>,
}

impl Merge3d {
    /// Whether the merge is in the scene at `ctx`'s frame.
    pub(crate) fn visible(&self, ctx: &crate::expr::ExpressionContext<'_>) -> crate::Result<bool> {
        crate::node::DelegateEvalContext::new(self.id, ctx)
            .eval_field("visible", &self.params.visible)
    }
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

/// What `render_3d` draws into a raster, each also an output of it of the same name. Depth,
/// normals and mattes hold data, with no transfer function applied. Depth and normals are drawn
/// from opaque surfaces and shadow catchers, masked surfaces where they reach their cutoff, and
/// blended ones, such as image planes, where their alpha reaches half. They skip lighting, shadows
/// and the environment, and ignore `supersample`: each pixel holds the nearest surface at its
/// centre, never an average across an edge. Mattes are drawn with the colour, and antialiased
/// with it.
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
    /// The visible coverage of the objects each of `mattes` names, one name to a channel: red,
    /// green, blue, then alpha. A surface covers its channel by its alpha as the colour shows
    /// it, so a surface `alpha_mode` `mask` cuts out covers none where it is cut, and a surface in
    /// front hides a channel behind it by its own alpha, as it hides colour: an object behind a
    /// plane at 0.5 opacity is 0.5 in its channel, and an opaque one in front leaves 0. A shadow
    /// catcher covers its channel by how much it darkens. Channels no name is for, and the alpha
    /// of a render with fewer than four, are 0. It is data rather than a picture, so the channels
    /// aren't premultiplied by the alpha: read one with `merge`'s `mask_mode` `red`, `green` or
    /// `blue`, or the alpha with `alpha`.
    Matte = 3,
}

#[cfg(feature = "3d")]
impl Render3dOutput {
    /// The outputs in the order `render_3d` names them, which their ports count.
    const PORTS: [Self; 4] = [Self::Color, Self::Depth, Self::Normal, Self::Matte];

    /// The port of `render_3d` that reads this output.
    pub(super) fn port(self) -> crate::node::OutputPort {
        let index = Self::PORTS.iter().position(|&output| output == self);
        crate::node::OutputPort::named(index.expect("every output has a port"))
    }

    /// The output a port `render_3d` names reads; `None` for its default `output`.
    pub(super) fn of_port(port: crate::node::OutputPort) -> Option<Self> {
        Self::PORTS.get(port.index()?).copied()
    }

    /// The outputs `ports`, ports `render_3d` names, read.
    pub(super) fn of_ports(ports: crate::node::OutputSet) -> impl Iterator<Item = Self> {
        ports.iter().filter_map(Self::of_port)
    }
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Render3dParams {
    /// What the default `output` holds: the lit scene, its depth, its normals or its mattes. The
    /// `color`, `depth`, `normal` and `matte` outputs each hold one whatever this is, and one
    /// `render_3d` draws all that are read with one walk of the scene, the colour and mattes with
    /// one draw of it.
    #[meta(constant)]
    pub output: Render3dOutput,
    /// The matte names whose objects' coverage the `matte` output holds, in its red, green, blue
    /// and alpha channels in turn: at most 4, each listed once, or the render fails. Objects take
    /// a name from their `matte` param, or from the `merge_3d`, `instance_3d` or `particles_3d`
    /// holding them. A name no object takes leaves its channel 0, with a `scene.matte_unused`
    /// warning when the `matte` output is drawn.
    #[meta()]
    pub mattes: MatteNames,
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
    /// Views through the camera's lens averaged for its depth of field, each a draw of the scene
    /// at the supersampled size: more is a smoother blur. Does nothing while the camera's `f_stop`
    /// is 0 or its projection is orthographic, and at 1. The `depth` and `normal` outputs are
    /// drawn from the centre of the lens, sharp.
    #[meta(min = 1, max = 64, enforced_min, step = 1)]
    pub dof_samples: u32,
    /// Exposure adjustment in stops before tone mapping.
    #[meta(step = 0.1)]
    pub exposure: f64,
    /// How light brighter than white is brought into range.
    #[meta()]
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
            mattes: MatteNames::default(),
            depth_near: 0.0,
            depth_far: 0.0,
            width: 0,
            height: 0,
            supersample: 2,
            dof_samples: 16,
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
/// map; or, by `output`, its depth or normals, for 2D effects such as fog, or the mattes of its
/// objects. Its `color`, `depth`, `normal` and `matte` outputs read each of them from the one
/// render.
///
/// A matte selects objects after rendering without drawing the scene again: name the lettering
/// `title` with its `matte` param, list `mattes: ["title"]`, and a `merge` whose `overlay` is the
/// `color` output, whose `base` is a `blur` of it, and whose `mask` is the `matte` output with
/// `mask_mode` `red` keeps the title sharp while the rest blurs. A matte's edges are antialiased
/// with the colour's, by `supersample`, so they line up.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "render_3d",
    name = "Render 3D",
    category = "scene",
    outputs(color, depth, normal, matte)
)]
pub struct Render3d {
    pub id: NodeId,
    #[params]
    pub params: Render3dParamsDelegate,

    #[input(scene)]
    pub scene: Option<OutputRef>,
}
