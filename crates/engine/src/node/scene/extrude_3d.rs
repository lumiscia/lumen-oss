//! `extrude_3d`: a 2D outline given depth.

use super::Placement3d;
#[cfg(feature = "3d")]
use super::text_3d::check_length;
use crate::node::{NodeId, NodeParams};

/// Most bevel steps.
#[cfg(feature = "3d")]
pub(super) const MAX_BEVEL_SEGMENTS: u32 = 16;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Extrude3dParams {
    /// World units that one pixel of the geometry spans, so that at the default of 0.01 a
    /// 100 pixel outline is 1 unit across. The geometry's own position, scale and rotation apply
    /// before it. 0 draws nothing. At most 1000000.
    #[meta(min = 0, enforced_min, step = 0.001)]
    pub units_per_pixel: f64,
    /// Thickness along Z in world units, centred on Z = 0 before `placement`, so the front faces
    /// +Z at half of it. 0 is flat.
    #[meta(min = 0, enforced_min, step = 0.05)]
    pub depth: f64,
    /// How far the bevel rounds off the outline's edges in world units, in from the outline and in
    /// from the front and back, up to half of `depth`; 0 is none. The outline keeps its size
    /// where the sides meet the bevel. Where the outline is too thin or too tightly curved for
    /// the bevel it gets the widest one that fits.
    #[meta(min = 0, enforced_min, step = 0.005)]
    pub bevel_size: f64,
    /// Steps of the bevel's quarter-round profile, up to 16: 1 is a flat 45° chamfer, and 0 is no
    /// bevel.
    #[meta(min = 0, max = 16, step = 1)]
    pub bevel_segments: u32,
    #[meta(flatten)]
    pub placement: Placement3d,
}

#[cfg(feature = "3d")]
impl Extrude3dParams {
    /// Fails with what is wrong with a number among the params, which are finite and, but for
    /// none, not negative.
    pub(super) fn check(&self) -> Result<(), String> {
        for (param, value) in [
            ("units_per_pixel", self.units_per_pixel),
            ("depth", self.depth),
            ("bevel_size", self.bevel_size),
        ] {
            check_length(param, value, false)?;
        }
        Ok(())
    }
}

impl Default for Extrude3dParams {
    fn default() -> Self {
        Self {
            units_per_pixel: 0.01,
            depth: 0.25,
            bevel_size: 0.0,
            bevel_segments: 4,
            placement: Placement3d::default(),
        }
    }
}

/// A 2D outline, a `path_2d`, `shape_2d` or `text_2d` wired into `geometry`, extruded into a solid:
/// a front cap facing +Z, a back cap facing -Z and side walls, lit, single-sided and white unless a
/// `material_3d` is wired into `material`. A `material_3d` wired into `side_material` covers the
/// side walls and the bevel instead, leaving the caps to `material`. The geometry's pixel coordinates, after its own
/// position, scale, rotation and anchor, become world X and Y at `units_per_pixel` units each, with
/// Y flipped, since 2D Y points down and 3D Y up. Pixel (0, 0) is the origin that `placement`
/// moves and turns the solid about, and nothing re-centres the solid on it: the geometry's
/// `position` and `anchor` decide where it sits about that origin, so a `shape_2d` at `position`
/// (0, 0) lies to the right of and below it on screen, and 100 pixels across it is 1 unit wide.
/// The extrusion is centred on Z = 0. The outline is filled by the nonzero rule, as a `text_3d`
/// glyph is, so holes of a path stay open whichever way they wind and overlapping subpaths join;
/// open subpaths are closed by a straight segment. Curves are flattened to within 0.001 world
/// units, so a larger or smaller `units_per_pixel` keeps them as smooth. An ellipse has as many
/// sides as that takes, at least 16 and at most 256, and each corner of a rounded rectangle a
/// quarter of as many for its own radius, at least 4 and at most 64. The flattening of a path or
/// a text coarsens where it would pass 16384 points. A solid takes at most 256 contours, 16384 points,
/// 262144 vertices and 786432 indices, and bevels fit outlines of at most 4096 points; more is a
/// render error. An `offset_2d` in the geometry is not supported yet and fails the render. The
/// mesh is built again only when the outline, `units_per_pixel` or the bevel change, or the
/// `depth` of a bevelled one, so animating the placement, the material or a plain extrusion's
/// depth costs no more than for a `shape_3d`.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "extrude_3d",
    name = "Extrude 3D",
    category = "scene",
    output = "scene"
)]
pub struct Extrude3d {
    pub id: NodeId,
    #[params]
    pub params: Extrude3dParamsDelegate,

    #[input(geometry)]
    pub geometry: Option<NodeId>,
    #[input(material, optional)]
    pub material: Option<NodeId>,
    #[input(material, optional)]
    pub side_material: Option<NodeId>,
}
