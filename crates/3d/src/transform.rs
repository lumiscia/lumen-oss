use glam::{EulerRot, Mat4, Quat, Vec3};

/// Where something sits in its parent's space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    /// Degrees about X, Y and Z, in the order `rotation` applies them.
    pub rotation: Vec3,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
        }
    }
}

impl Transform {
    /// Scales, then rotates, then moves to `position`.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, rotation(self.rotation), self.position)
    }
}

/// A rotation of `degrees` about X, Y and Z: yaw about Y, then pitch about the turned X, then
/// roll about the turned Z.
pub fn rotation(degrees: Vec3) -> Quat {
    Quat::from_euler(
        EulerRot::YXZ,
        degrees.y.to_radians(),
        degrees.x.to_radians(),
        degrees.z.to_radians(),
    )
}

/// The camera-to-world matrix of a camera at `eye` aimed at `target`, with Y up, or Z up when it
/// aims straight up or down. A target at the eye leaves the camera unturned.
pub fn look_at(eye: Vec3, target: Vec3) -> Mat4 {
    let forward = target - eye;
    if forward.length_squared() < 1e-12 {
        return Mat4::from_translation(eye);
    }
    let up = if forward.normalize().y.abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    Mat4::look_at_rh(eye, target, up).inverse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn rotation_yaws_then_pitches_then_rolls() {
        // Pitching +Z up by 90° points it down -Y, which a yaw about Y then leaves alone.
        let turned = rotation(Vec3::new(90.0, 90.0, 0.0)) * Vec3::Z;
        assert!(close(turned, Vec3::NEG_Y), "{turned}");
        // Rolling first would leave +Z where it is.
        let rolled = rotation(Vec3::new(0.0, 0.0, 90.0)) * Vec3::Z;
        assert!(close(rolled, Vec3::Z), "{rolled}");
        let yawed = rotation(Vec3::new(0.0, 90.0, 0.0)) * Vec3::Z;
        assert!(close(yawed, Vec3::X), "{yawed}");
    }

    #[test]
    fn a_transform_scales_then_rotates_then_moves() {
        let transform = Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Vec3::new(0.0, 90.0, 0.0),
            scale: Vec3::new(2.0, 1.0, 1.0),
        };
        let moved = transform.matrix().transform_point3(Vec3::X);
        assert!(close(moved, Vec3::new(1.0, 2.0, 1.0)), "{moved}");
    }

    #[test]
    fn look_at_aims_the_cameras_minus_z_at_the_target() {
        let eye = Vec3::new(3.0, 4.0, 5.0);
        let camera = look_at(eye, Vec3::ZERO);
        let forward = camera.transform_vector3(Vec3::NEG_Z);
        assert!(close(forward, -eye.normalize()), "{forward}");
        assert!(close(camera.transform_point3(Vec3::ZERO), eye));
        let down = look_at(Vec3::Y, Vec3::ZERO).transform_vector3(Vec3::NEG_Z);
        assert!(close(down, Vec3::NEG_Y), "{down}");
        assert_eq!(look_at(eye, eye), Mat4::from_translation(eye));
    }
}
