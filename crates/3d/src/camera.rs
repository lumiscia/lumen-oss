use glam::{Mat4, Vec3};

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
}
