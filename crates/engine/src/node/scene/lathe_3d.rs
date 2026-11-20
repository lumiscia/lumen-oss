//! `lathe_3d`: a 2D profile spun around an axis.

use super::Placement3d;
#[cfg(feature = "3d")]
use super::text_3d::check_length;
use crate::node::{NodeId, NodeParams};

/// Fewest and most steps around the axis.
#[cfg(feature = "3d")]
pub(super) const MIN_SEGMENTS: u32 = 3;
#[cfg(feature = "3d")]
pub(super) const MAX_SEGMENTS: u32 = 512;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Lathe3dParams {
    /// Steps around the axis for a whole turn, from 3 to 512; a `sweep` of less than 360° has
    /// proportionally fewer. The surface is shaded smoothly around the axis, so a few dozen look
    /// round.
    #[meta(min = 3, max = 512, step = 1)]
    pub segments: u32,
    /// How far around the axis the profile turns, in degrees, from 0 to 360, starting on +X and
    /// turning towards -Z. Under 360 leaves an open wedge with no end caps; 0 draws nothing.
    #[meta(min = 0, max = 360, step = 1)]
    pub sweep: f64,
    /// World units that one pixel of the geometry spans, so that at the default of 0.01 a
    /// 100 pixel profile is 1 unit across. The geometry's own position, scale and rotation apply
    /// before it. 0 draws nothing. At most 1000000.
    #[meta(min = 0, enforced_min, step = 0.001)]
    pub units_per_pixel: f64,
    #[meta(flatten)]
    pub placement: Placement3d,
}

#[cfg(feature = "3d")]
impl Lathe3dParams {
    /// Fails with what is wrong with a number among the params.
    pub(super) fn check(&self) -> Result<(), String> {
        check_length("units_per_pixel", self.units_per_pixel, false)?;
        if !self.sweep.is_finite() {
            return Err(format!(
                "`sweep` must be a finite number, got {}",
                self.sweep
            ));
        }
        Ok(())
    }
}

impl Default for Lathe3dParams {
    fn default() -> Self {
        Self {
            segments: 48,
            sweep: 360.0,
            units_per_pixel: 0.01,
            placement: Placement3d::default(),
        }
    }
}

/// A 2D profile, a `path_2d`, `shape_2d` or `text_2d` wired into `geometry`, spun around the world Y axis
/// into a solid of revolution, lit, single-sided and white unless a `material_3d` is wired into
/// `material`. The geometry's pixel coordinates, after its own position, scale, rotation and
/// anchor, become world units at `units_per_pixel` each: a point's X is its distance from the axis
/// and its Y, flipped since 2D Y points down and 3D Y up, its height, so pixel (0, 0) is on the
/// axis at height 0, which is the origin that `placement` moves and turns the solid about. Points
/// left of the axis are put on it. Each contour becomes a strip of quads around the axis, shaded
/// smoothly around it, and along the profile except at corners sharper than 30°, which stay hard.
/// A closed contour makes a closed ring solid whose faces point out of the area it encloses,
/// whichever way it winds. An open contour makes a surface whose front faces the right-hand side of
/// the direction it is drawn in, and which is seen from both sides only if its material is
/// `double_sided`. A point on the axis is a pole, where the surface closes to a point, such as the
/// tip of a cone or the top of a dome; the quads there are triangles, so no triangle without
/// area is left in the mesh, and a closed contour's edge along the axis draws nothing. Contours
/// are spun separately, so one inside another is a separate solid and not a hole. A solid takes
/// at most 256 contours, 16384 profile points, 262144 vertices and 786432 indices in all, which
/// is the profile's vertices times the steps around; more is a render error. The mesh is built
/// again only when the profile, `units_per_pixel`, `segments` or `sweep` change, so animating the
/// placement or the material costs no more than for a `shape_3d`.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "lathe_3d",
    name = "Lathe 3D",
    category = "scene",
    output = "scene"
)]
pub struct Lathe3d {
    pub id: NodeId,
    #[params]
    pub params: Lathe3dParamsDelegate,

    #[input(geometry)]
    pub geometry: Option<NodeId>,
    #[input(material, optional)]
    pub material: Option<NodeId>,
}
