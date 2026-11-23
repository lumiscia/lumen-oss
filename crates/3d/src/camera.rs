use glam::{Mat4, Vec2, Vec3};

/// Height of a full-frame camera's 36 × 24 mm sensor in metres, which gives a field of view its
/// focal length.
pub const SENSOR_HEIGHT: f32 = 0.024;

/// The focal length in metres of a full-frame camera with a vertical field of view of `fov_y`
/// radians.
pub fn focal_length(fov_y: f32) -> f32 {
    0.5 * SENSOR_HEIGHT / (0.5 * fov_y).tan()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// Farther things look smaller, over a vertical field of view in radians.
    Perspective { fov_y: f32 },
    /// Things keep their size at any distance, over a view this many world units tall.
    Orthographic { height: f32 },
}

/// A viewpoint, looking down its own -Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Camera to world.
    pub world: Mat4,
    pub projection: Projection,
    /// The nearest and farthest distances drawn.
    pub near: f32,
    pub far: f32,
    pub lens: Lens,
}

/// A thin lens, through whose aperture things nearer or farther than its focus distance blur.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lens {
    /// Diameter of the aperture in world units. 0 is a pinhole, which keeps everything sharp.
    pub aperture: f32,
    /// Distance along the view, in world units, that stays sharp. More than 0.
    pub focus_distance: f32,
    /// Sides of the aperture: a circle below 3, and otherwise a polygon of that many sides.
    pub blades: u32,
    /// Turn of the polygon in radians, clockwise as seen from behind the camera. At 0 one corner
    /// points up.
    pub rotation: f32,
}

impl Lens {
    pub const PINHOLE: Self = Self {
        aperture: 0.0,
        focus_distance: 5.0,
        blades: 0,
        rotation: 0.0,
    };

    /// `count` places spread evenly over the aperture, as offsets along the camera's X and Y in
    /// world units: a golden-angle spiral over the circle, or over the polygon with each side's
    /// share of it pushed out to that side. A single place is the centre. The same lens and
    /// count always give the same places.
    pub fn samples(&self, count: u32) -> Vec<Vec2> {
        const GOLDEN_ANGLE: f32 = 2.399_963_2;
        if count <= 1 {
            return vec![Vec2::ZERO; count as usize];
        }
        let radius = 0.5 * self.aperture;
        let side = std::f32::consts::TAU / self.blades.max(1) as f32;
        let half_side = 0.5 * side;
        // The first side's normal is half a side clockwise from the corner pointing up.
        let first_normal = std::f32::consts::FRAC_PI_2 - self.rotation - half_side;
        (0..count)
            .map(|index| {
                let along = ((index as f32 + 0.5) / count as f32).sqrt();
                let angle = index as f32 * GOLDEN_ANGLE;
                if self.blades < 3 {
                    return Vec2::from_angle(angle) * along * radius;
                }
                // The spiral fills the triangle between the centre and each half side by its
                // share of angle, so the angle is warped to that half side's share of area.
                let from_first = angle - first_normal;
                let normal = first_normal + side * (from_first / side).round();
                let share = (angle - normal) / half_side;
                let from_normal = (share * half_side.tan()).atan();
                let reach = half_side.cos() / from_normal.cos();
                Vec2::from_angle(normal + from_normal) * along * reach * radius
            })
            .collect()
    }
}

impl Default for Camera {
    /// 5 units along +Z, looking at the origin with a 40° field of view.
    fn default() -> Self {
        Self {
            world: Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0)),
            projection: Projection::Perspective {
                fov_y: 40f32.to_radians(),
            },
            near: 0.05,
            far: 1000.0,
            lens: Lens::PINHOLE,
        }
    }
}

impl Camera {
    /// World to camera.
    pub fn view(&self) -> Mat4 {
        self.world.inverse()
    }

    /// Camera to clip space for an image `aspect` times as wide as it is tall, with depth from 0
    /// at `near` to 1 at `far`.
    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        match self.projection {
            Projection::Perspective { fov_y } => {
                Mat4::perspective_rh(fov_y, aspect, self.near, self.far)
            }
            Projection::Orthographic { height } => {
                let (half_width, half_height) = (height * 0.5 * aspect, height * 0.5);
                Mat4::orthographic_rh(
                    -half_width,
                    half_width,
                    -half_height,
                    half_height,
                    self.near,
                    self.far,
                )
            }
        }
    }

    /// World to clip space from the place `offset` along the camera's right and up on its lens, in
    /// world units. The view is the centre's, sheared by [`Self::lens_shear`], so the plane at the
    /// lens's focus distance, square-on to the view, lands where it does from the centre.
    pub fn lens_view_projection(&self, aspect: f32, offset: Vec2) -> Mat4 {
        self.projection_matrix(aspect) * self.view() * self.lens_shear(offset)
    }

    /// Where the place `offset` on the lens is in world space, as [`Self::lens_view_projection`]
    /// takes it.
    pub fn lens_position(&self, offset: Vec2) -> Vec3 {
        let (right, up, _) = self.lens_axes();
        self.position() + right * offset.x + up * offset.y
    }

    /// The world map that takes the lens point at `offset` to the camera's position and keeps
    /// every point on the focus plane where it is.
    fn lens_shear(&self, offset: Vec2) -> Mat4 {
        let (right, up, forward) = self.lens_axes();
        let shift = right * offset.x + up * offset.y;
        let along = shift / self.lens.focus_distance;
        let linear = |axis: Vec3| (axis + along * forward.dot(axis)).extend(0.0);
        Mat4::from_cols(
            linear(Vec3::X),
            linear(Vec3::Y),
            linear(Vec3::Z),
            (-shift * (1.0 + forward.dot(self.position()) / self.lens.focus_distance)).extend(1.0),
        )
    }

    /// The world directions of the view's right, up and forward, orthonormal even when the
    /// camera's world matrix shears.
    fn lens_axes(&self) -> (Vec3, Vec3, Vec3) {
        let forward = self
            .world
            .transform_vector3(-Vec3::Z)
            .normalize_or(-Vec3::Z);
        let up = self.world.transform_vector3(Vec3::Y);
        let up = (up - forward * forward.dot(up)).normalize_or(Vec3::Y);
        (forward.cross(up), up, forward)
    }

    pub fn position(&self) -> Vec3 {
        self.world.transform_point3(Vec3::ZERO)
    }

    /// The camera's +Z in world space, pointing back towards the viewer.
    pub fn back(&self) -> Vec3 {
        self.world.transform_vector3(Vec3::Z).normalize_or(Vec3::Z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where `point` lands in normalized device coordinates.
    fn ndc(camera: &Camera, point: Vec3) -> Vec3 {
        (camera.projection_matrix(2.0) * camera.view()).project_point3(point)
    }

    #[test]
    fn perspective_shrinks_with_distance_and_orthographic_does_not() {
        let perspective = Camera::default();
        let orthographic = Camera {
            projection: Projection::Orthographic { height: 4.0 },
            ..perspective
        };
        let (near, far) = (Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, -5.0));
        assert!(ndc(&perspective, far).y < ndc(&perspective, near).y * 0.6);
        assert!((ndc(&orthographic, far).y - ndc(&orthographic, near).y).abs() < 1e-6);
        // Half the 4-unit view is 2 units up, and the 2:1 image is twice as wide.
        let corner = ndc(&orthographic, Vec3::new(4.0, 2.0, 0.0));
        assert!((corner.x - 1.0).abs() < 1e-6 && (corner.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn depth_runs_from_the_near_to_the_far_plane() {
        let camera = Camera::default();
        let at = |distance: f32| ndc(&camera, Vec3::new(0.0, 0.0, 5.0 - distance)).z;
        assert!(at(camera.near).abs() < 1e-5);
        assert!((at(camera.far) - 1.0).abs() < 1e-5);
        assert_eq!(camera.back(), Vec3::Z);
        assert_eq!(camera.position(), Vec3::new(0.0, 0.0, 5.0));
    }

    #[test]
    fn every_place_on_the_lens_sees_the_focus_plane_alike_and_nearer_things_apart() {
        let camera = Camera {
            world: crate::look_at(Vec3::new(1.0, 2.0, 6.0), Vec3::ZERO),
            lens: Lens {
                aperture: 0.4,
                focus_distance: 4.0,
                blades: 6,
                rotation: 0.3,
            },
            ..Camera::default()
        };
        let centre = camera.projection_matrix(2.0) * camera.view();
        let along = |distance: f32, side: f32| {
            camera
                .world
                .transform_point3(Vec3::new(side, 0.5 * side, -distance))
        };
        let samples = camera.lens.samples(16);
        assert_eq!(samples.len(), 16);
        let mut spread = 0.0f32;
        for offset in samples {
            assert!(
                offset.length() <= 0.2 + 1e-5,
                "{offset} is inside the aperture"
            );
            let through = camera.lens_view_projection(2.0, offset);
            let focused = along(4.0, 0.7);
            let moved = through.project_point3(focused) - centre.project_point3(focused);
            assert!(moved.truncate().abs().max_element() < 1e-5, "{moved}");
            let near = along(1.0, 0.1);
            let moved = through.project_point3(near) - centre.project_point3(near);
            spread = spread.max(moved.truncate().length());
        }
        assert!(spread > 0.05, "{spread}");
        assert_eq!(camera.lens.samples(1), vec![Vec2::ZERO]);
    }

    #[test]
    fn a_sheared_camera_keeps_a_circular_lens_and_a_square_on_focus_plane() {
        let lens = Lens {
            aperture: 0.4,
            focus_distance: 4.0,
            ..Lens::PINHOLE
        };
        let parent = Mat4::from_scale(Vec3::new(2.0, 1.0, 1.0));
        let local = Mat4::from_translation(Vec3::new(0.5, 1.0, 3.0));
        for (name, rotation) in [
            ("roll", Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4)),
            ("yaw", Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4)),
        ] {
            let camera = Camera {
                world: parent * local * rotation,
                lens,
                ..Camera::default()
            };
            let centre = camera.projection_matrix(2.0) * camera.view();
            assert_eq!(
                camera.lens_view_projection(2.0, Vec2::ZERO),
                centre,
                "{name}"
            );

            let (right, up, forward) = camera.lens_axes();
            let eye = camera.position();
            // Screen right is the camera's +X.
            assert!(
                right.dot(camera.world.transform_vector3(Vec3::X)) > 0.0,
                "{name}"
            );
            let on_focus = |side: f32, rise: f32| eye + forward * 4.0 + right * side + up * rise;
            let near = eye + forward * 1.0 + right * 0.1;

            let mut reach = Vec2::ZERO;
            let mut spread = 0.0f32;
            for offset in [
                Vec2::new(0.2, 0.0),
                Vec2::new(-0.2, 0.0),
                Vec2::new(0.0, 0.2),
                Vec2::new(0.0, -0.2),
                Vec2::new(0.1, -0.15),
            ] {
                let lens_offset = camera.lens_position(offset) - eye;
                assert!(lens_offset.length() <= 0.2 + 1e-5, "{name} {lens_offset}");
                reach = reach.max(Vec2::new(lens_offset.dot(right), lens_offset.dot(up)).abs());

                let through = camera.lens_view_projection(2.0, offset);
                for (side, rise) in [(0.0, 0.0), (1.0, 0.0), (-2.0, 1.5), (1.5, -1.0)] {
                    let point = on_focus(side, rise);
                    let moved = through.project_point3(point) - centre.project_point3(point);
                    assert!(
                        moved.truncate().abs().max_element() < 1e-5,
                        "{name} {moved}"
                    );
                }
                let moved = through.project_point3(near) - centre.project_point3(near);
                spread = spread.max(moved.truncate().length());
            }
            assert!(
                (reach.x - 0.2).abs() < 1e-5 && (reach.y - 0.2).abs() < 1e-5,
                "{name} {reach}"
            );
            assert!(spread > 0.02, "{name} {spread}");
        }
    }
}
