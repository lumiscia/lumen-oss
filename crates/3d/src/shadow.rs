//! Shadow maps: the views shadow-casting lights see their casters from, one for a directional or
//! spot light and six for a point light, each a square tile of one depth atlas.

use glam::{Mat4, Vec2, Vec3, Vec4};

use crate::{Bounds, Camera, Light, LightKind, Projection};

/// Lights asking for shadows past this many, in scene order, cast none.
pub const MAX_SHADOW_LIGHTS: usize = 8;
/// The widest an atlas grows, which a single view fills.
const MAX_ATLAS_SIDE: u32 = 4096;
/// The widest a shadow edge blurs, in texels of its map, which a point light's faces overlap by.
pub const MAX_FILTER_TEXELS: f32 = 16.0;
/// The widest a spot light's shadow map reaches, in radians. Its cone past this is unshadowed.
const MAX_SPOT_FOV: f32 = 170.0 * std::f32::consts::PI / 180.0;
/// Steps per doubling that a directional view's width rounds up to, so the width only changes
/// when the view it fits grows or shrinks past a step.
const WIDTH_STEPS: f32 = 8.0;
/// The softness a directional view's reach is worked out for when its light's shadows are sharper.
const SHARPEST_REACH_SOFTNESS: f32 = 0.01;

/// One view a light casts shadows from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowView {
    /// World to the view's clip space, whose X and Y span its tile from -1 to 1.
    pub view_proj: Mat4,
    /// A world point `p` lies at depth `depth.xyz · p + depth.w` from the light, which is 1 at
    /// the view's far end and below 0 in front of the view.
    pub depth: Vec4,
    /// The world width of a texel; for a perspective view, at depth 1, and in proportion to depth.
    pub texel: f32,
    pub perspective: bool,
}

/// The shadow views of a scene's lights, laid out in a grid of square tiles.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowAtlas {
    /// The width and height of each tile in texels.
    pub tile: u32,
    /// Views fill the tiles row by row.
    pub columns: u32,
    pub rows: u32,
    pub views: Vec<ShadowView>,
    /// Each light's first view, or `None` for a light without shadows.
    pub first_view: Vec<Option<u32>>,
    /// The lights asking for shadows past the first [`MAX_SHADOW_LIGHTS`], which cast none.
    pub dropped: Vec<usize>,
}

impl ShadowAtlas {
    /// The views the first [`MAX_SHADOW_LIGHTS`] of `lights` asking for shadows cast them from,
    /// for casters within `casters` onto receivers within `receivers` that `camera` sees with an
    /// image `aspect` times as wide as it is tall, in an atlas no wider or taller than `max_side`.
    /// Lights whose place or direction isn't finite cast none, and don't count. `None` when no
    /// light casts shadows.
    pub fn plan(
        lights: &[Light],
        camera: &Camera,
        aspect: f32,
        casters: Bounds,
        receivers: Bounds,
        max_side: u32,
    ) -> Option<Self> {
        if casters.is_empty() || receivers.is_empty() {
            return None;
        }
        let mut casting = Vec::new();
        let mut dropped = Vec::new();
        for (index, light) in lights.iter().enumerate() {
            let asks = light.shadow.is_some()
                && view_count(light.kind) > 0
                && light.position.is_finite()
                && light.direction.is_finite();
            if !asks {
                continue;
            }
            if casting.len() < MAX_SHADOW_LIGHTS {
                casting.push(index);
            } else {
                dropped.push(index);
            }
        }
        let count: u32 = casting.iter().map(|&i| view_count(lights[i].kind)).sum();
        if count == 0 {
            return None;
        }
        let columns = (count as f32).sqrt().ceil() as u32;
        let rows = count.div_ceil(columns);
        // Even, so that half a view's width is a whole number of texels.
        let tile = (MAX_ATLAS_SIDE.min(max_side) / columns.max(rows)) & !1;
        if tile < 16 {
            return None;
        }
        let seen = Seen::of(camera, aspect, receivers);
        let mut views = Vec::with_capacity(count as usize);
        let mut first_view = vec![None; lights.len()];
        for index in casting {
            let light_views = views_of(&lights[index], seen.as_ref(), casters, receivers, tile);
            if let Some(light_views) = light_views {
                first_view[index] = Some(views.len() as u32);
                views.extend(light_views);
            }
        }
        if views.is_empty() {
            return None;
        }
        Some(Self {
            tile,
            columns,
            rows,
            views,
            first_view,
            dropped,
        })
    }

    /// The atlas's width and height in texels.
    pub fn size(&self) -> (u32, u32) {
        (self.columns * self.tile, self.rows * self.tile)
    }

    /// View `index`'s tile as fractions of the atlas: left, top, right and bottom.
    pub fn tile_rect(&self, index: usize) -> [f32; 4] {
        let (width, height) = self.size();
        let (column, row) = (index as u32 % self.columns, index as u32 / self.columns);
        let tile = self.tile as f32;
        [
            column as f32 * tile / width as f32,
            row as f32 * tile / height as f32,
            (column + 1) as f32 * tile / width as f32,
            (row + 1) as f32 * tile / height as f32,
        ]
    }
}

fn view_count(kind: LightKind) -> u32 {
    match kind {
        LightKind::Directional | LightKind::Spot => 1,
        LightKind::Point => 6,
        LightKind::Ambient => 0,
    }
}

/// An up direction for a view looking along `direction`.
fn up_for(direction: Vec3) -> Vec3 {
    if direction.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    }
}

/// `light`'s views of `casters` onto `receivers`, of which the camera sees what `seen` holds, or
/// `None` when any isn't finite.
fn views_of(
    light: &Light,
    seen: Option<&Seen>,
    casters: Bounds,
    receivers: Bounds,
    tile: u32,
) -> Option<Vec<ShadowView>> {
    // As far as the farthest receiver the camera sees, or the light's range when that ends sooner.
    let seen_points = seen.map_or(&[][..], |seen| seen.points.as_slice());
    let points = if seen_points.is_empty() {
        &receivers.corners()[..]
    } else {
        seen_points
    };
    let reach = points
        .iter()
        .map(|point| point.distance(light.position))
        .fold(0.0, f32::max)
        * 1.01;
    let far = if light.range > 0.0 {
        reach.min(light.range)
    } else {
        reach
    }
    .max(1e-3);
    // From just short of the nearest caster. Depth is written as distance over `far`, so `near`
    // only clips and costs no precision however small it is.
    let nearest = casters
        .min
        .max(light.position)
        .min(casters.max)
        .distance(light.position);
    let near = (nearest * 0.99).clamp(far * 1e-6, far * 0.5);
    let views = match light.kind {
        LightKind::Directional => vec![directional(light, seen, casters, receivers, tile)],
        LightKind::Spot => {
            let cone = 2.0 * light.cos_outer.clamp(-1.0, 1.0).acos();
            let fov = cone.clamp(1f32.to_radians(), MAX_SPOT_FOV);
            vec![perspective(
                light.position,
                light.direction,
                fov,
                near,
                far,
                tile,
            )]
        }
        LightKind::Point => {
            // Wider than a quarter turn by the filter's reach at each edge, so that filtering
            // near the edge of a face stays within it.
            let fov = 2.0 * (1.0 + 2.0 * (MAX_FILTER_TEXELS + 2.0) / tile as f32).atan();
            [
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Y,
                Vec3::NEG_Y,
                Vec3::Z,
                Vec3::NEG_Z,
            ]
            .map(|direction| perspective(light.position, direction, fov, near, far, tile))
            .to_vec()
        }
        LightKind::Ambient => return None,
    };
    let finite = |view: &ShadowView| {
        view.view_proj.is_finite()
            && view.depth.is_finite()
            && view.texel.is_finite()
            && view.texel > 0.0
    };
    views.iter().all(finite).then_some(views)
}

/// A view from `position` along `direction`, `fov` radians across, from `near` to `far`.
fn perspective(
    position: Vec3,
    direction: Vec3,
    fov: f32,
    near: f32,
    far: f32,
    tile: u32,
) -> ShadowView {
    let view = Mat4::look_to_rh(position, direction, up_for(direction));
    ShadowView {
        view_proj: Mat4::perspective_rh(fov, 1.0, near, far) * view,
        depth: -view.row(2) / far,
        texel: 2.0 * (fov * 0.5).tan() * far / tile as f32,
        perspective: true,
    }
}

/// A view along `light`'s direction covering the receivers within `receivers` that the camera
/// sees, as `seen` holds, from as far toward the light as `casters` in front of them reach. Where
/// that is wider than the view's map covers with texels as wide as the light's softness, or
/// [`SHARPEST_REACH_SOFTNESS`] when that is smaller, it covers only what is nearest the camera.
/// The view's width rounds up to one of a set of steps, and its centre to a whole texel in a frame
/// fixed to the world, so that while the camera moves, each texel stays on the same part of the
/// scene and edges don't shimmer.
fn directional(
    light: &Light,
    seen: Option<&Seen>,
    casters: Bounds,
    receivers: Bounds,
    tile: u32,
) -> ShadowView {
    let direction = light.direction;
    let turn = Mat4::look_to_rh(Vec3::ZERO, direction, up_for(direction));
    let softness = light
        .shadow
        .map_or(0.0, |shadow| shadow.softness)
        .max(SHARPEST_REACH_SOFTNESS);
    let fit = seen
        .and_then(|seen| seen.fit(&turn, softness * tile as f32))
        .unwrap_or_else(|| receivers.transformed(&turn));
    let extent = (fit.max - fit.min).truncate().max_element().max(1e-3);
    let margin = 1.0 + 2.0 * (MAX_FILTER_TEXELS + 2.0) / tile as f32;
    let width = ((extent * margin).log2() * WIDTH_STEPS).ceil() / WIDTH_STEPS;
    let width = width.exp2();
    let texel = width / tile as f32;
    let center = ((fit.min + fit.max).truncate() * 0.5 / texel).round() * texel;
    let half = width * 0.5;
    // The casters within the view's sides, between the farthest and nearest anything reaches.
    let scene = casters.union(receivers).transformed(&turn);
    let to_world = turn.inverse();
    let prism = [0, 1, 2, 3, 4, 5, 6, 7].map(|corner| {
        let side = |bit: usize| if corner & bit == 0 { -half } else { half };
        let z = if corner & 4 == 0 {
            scene.min.z
        } else {
            scene.max.z
        };
        to_world.transform_point3((center + Vec2::new(side(1), side(2))).extend(z))
    });
    let blocking = Bounds::of(
        within(&prism, &casters)
            .into_iter()
            .map(|point| turn.transform_point3(point)),
    );
    let depths = fit.union(blocking);
    let pad = texel * 4.0;
    // The light looks down its -Z, so the nearest casters have the greatest Z.
    let (near, far) = (-depths.max.z - pad, -depths.min.z + pad);
    let projection = Mat4::orthographic_rh(
        center.x - half,
        center.x + half,
        center.y - half,
        center.y + half,
        near,
        far,
    );
    ShadowView {
        view_proj: projection * turn,
        depth: (-turn.row(2) - Vec4::new(0.0, 0.0, 0.0, near)) / (far - near),
        texel,
        perspective: false,
    }
}

/// What the camera sees of the receivers.
struct Seen<'a> {
    camera: &'a Camera,
    /// The image's width over its height.
    aspect: f32,
    receivers: Bounds,
    /// The nearest and farthest distances along the camera's view of the receivers' corners.
    near: f32,
    far: f32,
    /// Points whose bounds are those of the receivers the camera sees.
    points: Vec<Vec3>,
}

impl<'a> Seen<'a> {
    /// What `camera`, with an image `aspect` times as wide as it is tall, sees of `receivers`, or
    /// `None` when that is nothing.
    fn of(camera: &'a Camera, aspect: f32, receivers: Bounds) -> Option<Self> {
        let view = camera.view();
        let depths = receivers
            .corners()
            .map(|corner| -view.transform_point3(corner).z);
        let near = depths.into_iter().fold(f32::INFINITY, f32::min);
        let far = depths.into_iter().fold(f32::NEG_INFINITY, f32::max);
        let (near, far) = (near.max(camera.near), far.min(camera.far));
        // Neither is NaN, and something lies between them.
        if near.partial_cmp(&far) != Some(std::cmp::Ordering::Less) {
            return None;
        }
        let mut seen = Self {
            camera,
            aspect,
            receivers,
            near,
            far,
            points: Vec::new(),
        };
        seen.points = seen.until(far);
        (!seen.points.is_empty()).then_some(seen)
    }

    /// Points whose bounds are those of the receivers seen no farther than `far` along the view.
    fn until(&self, far: f32) -> Vec<Vec3> {
        frustum(self.camera, self.aspect, self.near, far)
            .map(|frustum| within(&frustum, &self.receivers))
            .unwrap_or_default()
    }

    /// The bounds of the receivers seen, in the space `turn` takes the world to. When those are
    /// wider or taller than `reach`, only of those nearest the camera, as far as they stay within
    /// it.
    fn fit(&self, turn: &Mat4, reach: f32) -> Option<Bounds> {
        let fit = |points: &[Vec3]| {
            let fit = Bounds::of(points.iter().map(|&point| turn.transform_point3(point)));
            (!fit.is_empty()).then_some(fit)
        };
        let width = |fit: &Bounds| (fit.max - fit.min).truncate().max_element();
        let all = fit(&self.points)?;
        if width(&all) <= reach {
            return Some(all);
        }
        // The farthest distance whose fit stays within `reach`, which grows with the distance.
        let (mut inside, mut outside) = (self.near, self.far);
        for _ in 0..24 {
            let middle = 0.5 * (inside + outside);
            if fit(&self.until(middle)).is_none_or(|fit| width(&fit) <= reach) {
                inside = middle;
            } else {
                outside = middle;
            }
        }
        fit(&self.until(inside)).or_else(|| fit(&self.until(outside)))
    }
}

/// The corners of what `camera`, with an image `aspect` times as wide as it is tall, sees from
/// `near` to `far` along its view, ordered as [`Bounds::corners`] orders a box's: bit 0 of the
/// index is right, bit 1 up and bit 2 far. `None` when any isn't finite.
fn frustum(camera: &Camera, aspect: f32, near: f32, far: f32) -> Option<[Vec3; 8]> {
    let half_height = |distance: f32| match camera.projection {
        Projection::Perspective { fov_y } => distance * (fov_y * 0.5).tan(),
        Projection::Orthographic { height } => height * 0.5,
    };
    let corners = [0, 1, 2, 3, 4, 5, 6, 7].map(|corner| {
        let distance = if corner & 4 == 0 { near } else { far };
        let y = half_height(distance) * if corner & 2 == 0 { -1.0 } else { 1.0 };
        let x = y.abs() * aspect * if corner & 1 == 0 { -1.0 } else { 1.0 };
        camera.world.transform_point3(Vec3::new(x, y, -distance))
    });
    corners
        .iter()
        .all(|corner| corner.is_finite())
        .then_some(corners)
}

/// Points whose bounds are those of where the `frustum` and the box `bounds` overlap: where the
/// edges of each cross into the other. Every corner of the overlap is one of them, but for those
/// too far out for `f32`, which are left out.
fn within(frustum: &[Vec3; 8], bounds: &Bounds) -> Vec<Vec3> {
    let centre = frustum.iter().sum::<Vec3>() / 8.0;
    // Each face's plane, as a normal pointing inward and an offset, from three of its corners.
    let faces: [(usize, usize); 6] = [(1, 0), (1, 1), (2, 0), (2, 1), (4, 0), (4, 1)];
    let frustum_planes = faces.map(|(bit, side)| {
        let on: Vec<Vec3> = (0..8)
            .filter(|corner| (corner & bit != 0) == (side == 1))
            .map(|corner| frustum[corner])
            .collect();
        let normal = (on[1] - on[0]).cross(on[2] - on[0]);
        let normal = if normal.dot(centre - on[0]) < 0.0 {
            -normal
        } else {
            normal
        };
        normal.extend(-normal.dot(on[0]))
    });
    let box_planes = [
        Vec3::X.extend(-bounds.min.x),
        Vec3::NEG_X.extend(bounds.max.x),
        Vec3::Y.extend(-bounds.min.y),
        Vec3::NEG_Y.extend(bounds.max.y),
        Vec3::Z.extend(-bounds.min.z),
        Vec3::NEG_Z.extend(bounds.max.z),
    ];
    let mut points = Vec::new();
    for (corners, planes) in [(frustum, &box_planes), (&bounds.corners(), &frustum_planes)] {
        for (from, to) in edges().map(|(a, b)| (corners[a], corners[b])) {
            if let Some((from, to)) = clip(from, to, planes) {
                points.extend([from, to]);
            }
        }
    }
    points.retain(|point| point.is_finite());
    points
}

/// The 12 edges of a box whose corners are ordered as [`Bounds::corners`] orders them.
fn edges() -> impl Iterator<Item = (usize, usize)> {
    (0..8).flat_map(|corner: usize| {
        [1, 2, 4]
            .into_iter()
            .filter(move |bit| corner & bit == 0)
            .map(move |bit| (corner, corner | bit))
    })
}

/// The part of the segment from `from` to `to` on the inner side of every plane, if any. A
/// segment too far out for `f32` to say has none.
fn clip(from: Vec3, to: Vec3, planes: &[Vec4; 6]) -> Option<(Vec3, Vec3)> {
    let (mut start, mut end) = (0.0f32, 1.0f32);
    for plane in planes {
        let a = plane.truncate().dot(from) + plane.w;
        let b = plane.truncate().dot(to) + plane.w;
        if !(a.is_finite() && b.is_finite()) || (a < 0.0 && b < 0.0) {
            return None;
        }
        if a < 0.0 {
            start = start.max(a / (a - b));
        } else if b < 0.0 {
            end = end.min(a / (a - b));
        }
    }
    // A tolerance keeps the edges of a box with no thickness, which cross in a point.
    (start <= end + 1e-5).then(|| (from.lerp(to, start), from.lerp(to, end.max(start))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(kind: LightKind, world: Mat4) -> Light {
        Light {
            shadow: Some(crate::Shadow { softness: 0.02 }),
            ..Light::new(kind, Vec3::ONE, world, 0.0, 60.0, 0.2)
        }
    }

    /// A light at `position` aimed at the origin.
    fn aimed(kind: LightKind, position: Vec3) -> Light {
        light(kind, crate::look_at(position, Vec3::ZERO))
    }

    /// A 20 m floor with a metre-tall caster on it.
    fn scene() -> Bounds {
        Bounds {
            min: Vec3::new(-10.0, 0.0, -10.0),
            max: Vec3::new(10.0, 1.0, 10.0),
        }
    }

    /// Where `view` puts `point` in its tile, in texels, and its depth.
    fn texel_of(view: &ShadowView, tile: u32, point: Vec3) -> Vec3 {
        let ndc = view.view_proj.project_point3(point);
        let half = tile as f32 * 0.5;
        Vec3::new(
            (ndc.x + 1.0) * half,
            (1.0 - ndc.y) * half,
            view.depth.truncate().dot(point) + view.depth.w,
        )
    }

    /// A floor `half` units out each way with a metre box on it.
    fn ground(half: f32) -> Bounds {
        Bounds {
            min: Vec3::new(-half, 0.0, -half),
            max: Vec3::new(half, 0.0, half),
        }
        .union(Bounds::of([
            Vec3::splat(-0.5).with_y(0.0),
            Vec3::splat(0.5).with_y(1.0),
        ]))
    }

    #[test]
    fn a_directional_view_fits_what_the_camera_sees_of_a_floor_whatever_its_size() {
        let sun = aimed(LightKind::Directional, Vec3::new(2.0, 5.0, 3.0));
        let camera = Camera {
            world: crate::look_at(Vec3::new(0.0, 3.0, 5.0), Vec3::ZERO),
            ..Camera::default()
        };
        let mark = Vec3::new(0.3, 1.0, -0.2);
        let fits = [20.0, 500.0, 5000.0].map(|half| {
            let scene = ground(half);
            let atlas = ShadowAtlas::plan(&[sun], &camera, 16.0 / 9.0, scene, scene, 8192).unwrap();
            let view = atlas.views[0];
            (view.texel, texel_of(&view, atlas.tile, mark).truncate())
        });
        // The camera sees the floor to about 15 m away.
        assert!(fits[0].0 * 4096.0 < 40.0, "{}", fits[0].0 * 4096.0);
        for fit in &fits[1..] {
            assert_eq!(fit.0, fits[0].0);
            assert!(
                (fit.1 - fits[0].1).abs().max_element() < 1e-2,
                "{fit:?} {:?}",
                fits[0]
            );
        }
    }

    #[test]
    fn a_directional_view_of_a_floor_to_the_horizon_keeps_to_what_its_map_resolves_near_the_camera()
    {
        let sun = aimed(LightKind::Directional, Vec3::new(2.0, 5.0, 3.0));
        let camera = Camera {
            world: crate::look_at(Vec3::new(0.0, 1.5, 5.0), Vec3::new(0.0, 1.5, 0.0)),
            ..Camera::default()
        };
        let scene = ground(5000.0);
        let atlas = ShadowAtlas::plan(&[sun], &camera, 16.0 / 9.0, scene, scene, 8192).unwrap();
        let view = atlas.views[0];
        // Texels no wider than the light's softness, give or take the width's rounding and margin.
        assert!(view.texel < 0.02 * 1.2, "{}", view.texel);
        let box_top = texel_of(&view, atlas.tile, Vec3::Y);
        assert!(box_top.x > 0.0 && box_top.x < 4096.0 && box_top.y > 0.0 && box_top.y < 4096.0);
    }

    #[test]
    fn spot_and_point_views_reach_from_the_nearest_caster_to_the_farthest_receiver() {
        let scene = ground(5000.0);
        let spot = aimed(LightKind::Spot, Vec3::new(0.0, 4.0, 0.0));
        let point = light(LightKind::Point, Mat4::from_translation(Vec3::Y * 3.0));
        let atlas =
            ShadowAtlas::plan(&[spot, point], &Camera::default(), 1.0, scene, scene, 8192).unwrap();
        // The box's top is 3 m below the spot light and 2 m below the point light.
        for view in [atlas.views[0], atlas.views[1 + 3]] {
            let clip = view.view_proj * Vec3::Y.extend(1.0);
            assert!(clip.z >= 0.0 && clip.z <= clip.w, "{clip}");
            let corner = view.view_proj * Vec3::new(4999.0, 0.0, 4999.0).extend(1.0);
            assert!(corner.z <= corner.w, "{corner}");
        }
    }

    #[test]
    fn a_directional_view_covers_what_the_camera_sees_and_orders_depth_from_the_light() {
        let sun = aimed(LightKind::Directional, Vec3::new(2.0, 5.0, 3.0));
        let camera = Camera::default();
        let atlas = ShadowAtlas::plan(&[sun], &camera, 1.0, scene(), scene(), 8192).unwrap();
        assert_eq!((atlas.tile, atlas.size()), (4096, (4096, 4096)));
        let view = atlas.views[0];
        // The camera, 5 m out with a 40° view, sees the floor to 15 m away, so the view is
        // narrower than the floor, which is about 28 m across seen from the light.
        assert!(view.texel * 4096.0 < 20.0, "{}", view.texel * 4096.0);
        let top = texel_of(&view, atlas.tile, Vec3::new(0.0, 1.0, 0.0));
        let floor = texel_of(&view, atlas.tile, Vec3::ZERO);
        for at in [top, floor] {
            assert!(
                at.x > 0.0 && at.x < 4096.0 && at.y > 0.0 && at.y < 4096.0,
                "{at}"
            );
            assert!(at.z > 0.0 && at.z < 1.0, "{at}");
        }
        assert!(top.z < floor.z);
    }

    #[test]
    fn a_directional_views_texels_stay_on_the_scene_while_the_camera_moves() {
        let sun = aimed(LightKind::Directional, Vec3::new(2.0, 5.0, 3.0));
        let marks = [Vec3::new(0.3, 0.0, -0.7), Vec3::new(-1.1, 0.5, 0.2)];
        let mut first = None;
        for step in 0..12 {
            let camera = Camera {
                world: crate::look_at(
                    Vec3::new(step as f32 * 0.0137, 2.0, 5.0),
                    Vec3::new(step as f32 * 0.0137, 0.0, 0.0),
                ),
                ..Camera::default()
            };
            let atlas =
                ShadowAtlas::plan(&[sun], &camera, 16.0 / 9.0, scene(), scene(), 8192).unwrap();
            let view = atlas.views[0];
            let fractions = marks.map(|mark| texel_of(&view, atlas.tile, mark).truncate().fract());
            let (texel, start) = *first.get_or_insert((view.texel, fractions));
            assert_eq!(view.texel, texel, "step {step}");
            for (now, then) in fractions.iter().zip(start) {
                let moved = (*now - then).abs();
                assert!(
                    moved.min(1.0 - moved).max_element() < 2e-3,
                    "step {step}: {now} from {then}"
                );
            }
        }
    }

    #[test]
    fn spot_and_point_lights_see_from_where_they_are() {
        let spot = aimed(LightKind::Spot, Vec3::new(0.0, 4.0, 0.0));
        let point = light(LightKind::Point, Mat4::from_translation(Vec3::Y * 3.0));
        let atlas = ShadowAtlas::plan(
            &[spot, point],
            &Camera::default(),
            1.0,
            scene(),
            scene(),
            8192,
        )
        .unwrap();
        assert_eq!(atlas.views.len(), 7);
        assert_eq!(atlas.first_view, [Some(0), Some(1)]);
        assert_eq!((atlas.columns, atlas.rows), (3, 3));
        assert_eq!(atlas.tile, 1364);
        let below = texel_of(&atlas.views[0], atlas.tile, Vec3::ZERO);
        assert!(
            (below.x - 682.0).abs() < 1e-2 && (below.y - 682.0).abs() < 1e-2,
            "{below}"
        );
        // Depth is distance along the view over its reach, to the farthest part of the floor
        // the camera sees.
        let reach = 1.0 / atlas.views[0].depth.truncate().length();
        assert!((below.z - 4.0 / reach).abs() < 1e-5, "{below}");
        assert!(reach > 4.0 && reach <= Vec3::new(10.0, -4.0, 10.0).length() * 1.01);
        // Each point lies in the face of the point light it is most in the direction of.
        for (face, toward) in [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .into_iter()
        .enumerate()
        {
            let at = texel_of(&atlas.views[1 + face], atlas.tile, Vec3::Y * 3.0 + toward);
            let middle = atlas.tile as f32 * 0.5;
            assert!(
                (at.x - middle).abs() < 1e-2 && (at.y - middle).abs() < 1e-2,
                "{face}: {at}"
            );
        }
    }

    #[test]
    fn only_the_first_lights_asking_for_shadows_cast_them() {
        let sun = aimed(LightKind::Directional, Vec3::new(2.0, 5.0, 3.0));
        let unshadowed = Light {
            shadow: None,
            ..sun
        };
        let lost = Light {
            position: Vec3::NAN,
            ..aimed(LightKind::Spot, Vec3::Y)
        };
        let mut lights = vec![unshadowed, lost];
        lights.extend([sun; MAX_SHADOW_LIGHTS + 2]);
        let atlas =
            ShadowAtlas::plan(&lights, &Camera::default(), 1.0, scene(), scene(), 8192).unwrap();
        assert_eq!(atlas.views.len(), MAX_SHADOW_LIGHTS);
        assert_eq!(atlas.first_view[..3], [None, None, Some(0)]);
        assert_eq!(atlas.dropped, [10, 11]);
        let (width, height) = atlas.size();
        assert!(width <= 4096 && height <= 4096);

        assert_eq!(
            ShadowAtlas::plan(
                &[sun],
                &Camera::default(),
                1.0,
                Bounds::EMPTY,
                scene(),
                8192
            ),
            None
        );
        assert_eq!(
            ShadowAtlas::plan(
                &[unshadowed],
                &Camera::default(),
                1.0,
                scene(),
                scene(),
                8192
            ),
            None
        );
        let small =
            ShadowAtlas::plan(&[sun], &Camera::default(), 1.0, scene(), scene(), 1000).unwrap();
        assert_eq!(small.tile, 1000);
    }

    #[test]
    fn views_of_a_scene_too_large_for_f32_are_finite_or_left_out() {
        let sun = aimed(LightKind::Directional, Vec3::new(2.0, 5.0, 3.0));
        let point = light(LightKind::Point, Mat4::IDENTITY);
        let far = light(LightKind::Point, Mat4::from_translation(Vec3::splat(3e38)));
        let huge = Bounds {
            min: Vec3::splat(-3e38),
            max: Vec3::splat(3e38),
        };
        let atlas = ShadowAtlas::plan(
            &[sun, point, far],
            &Camera::default(),
            1.0,
            huge,
            huge,
            8192,
        )
        .unwrap();
        assert_eq!(atlas.first_view, [Some(0), Some(1), None]);
        for view in atlas.views {
            assert!(view.view_proj.is_finite() && view.depth.is_finite() && view.texel > 0.0);
        }
        assert_eq!(
            ShadowAtlas::plan(&[far], &Camera::default(), 1.0, huge, huge, 8192),
            None
        );
    }
}
