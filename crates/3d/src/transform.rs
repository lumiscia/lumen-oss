use glam::{DMat3, EulerRot, Mat3, Mat4, Quat, Vec3};

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

/// Whether `world` turns things inside out, as a negative scale on one axis does, so that each
/// face's vertices wind the other way round.
pub fn mirrors(world: &Mat4) -> bool {
    // In f64, where the product of three f32s can't overflow or underflow to zero.
    Mat3::from_mat4(*world).as_dmat3().determinant() < 0.0
}

/// What takes a mesh's normals to world space under `world`: the inverse transpose up to a
/// positive factor, scaled so that its largest entry is 1, so the normals it gives need
/// normalising. Unlike the inverse transpose, it stays defined when `world` flattens one axis,
/// which leaves the faces across that axis their area. `None` when `world` is not finite or leaves
/// no face any area.
pub fn normal_matrix(world: &Mat4) -> Option<Mat3> {
    if !world.is_finite() {
        return None;
    }
    // The cofactor matrix, which is the determinant times the inverse transpose, in f64, where
    // products of two f32s can't overflow or underflow to zero.
    let DMat3 {
        x_axis: x,
        y_axis: y,
        z_axis: z,
    } = Mat3::from_mat4(*world).as_dmat3();
    let cofactor = DMat3::from_cols(y.cross(z), z.cross(x), x.cross(y));
    let largest = cofactor
        .abs()
        .to_cols_array()
        .into_iter()
        .fold(0.0, f64::max);
    if largest == 0.0 {
        return None;
    }
    let determinant = x.dot(cofactor.x_axis);
    let sign = if determinant < 0.0 { -1.0 } else { 1.0 };
    Some((cofactor * (sign / largest)).as_mat3())
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

    /// `normal` in world space under `world`, at unit length as the scene shader takes it.
    fn turned(world: Mat4, normal: Vec3) -> Vec3 {
        let turned = normal_matrix(&world).expect("a surface with area") * normal;
        assert!(turned.is_finite(), "{turned}");
        (turned / turned.abs().max_element()).normalize()
    }

    #[test]
    fn normals_turn_as_the_inverse_transpose_turns_them() {
        let world = Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Vec3::new(20.0, 30.0, 40.0),
            scale: Vec3::new(2.0, 0.5, 3.0),
        }
        .matrix();
        let inverse_transpose = Mat3::from_mat4(world).inverse().transpose();
        for normal in [Vec3::X, Vec3::Y, Vec3::new(1.0, -2.0, 0.5).normalize()] {
            let expected = (inverse_transpose * normal).normalize();
            assert!(close(turned(world, normal), expected), "{normal}");
        }
    }

    #[test]
    fn normals_keep_facing_out_of_mirrored_and_flattened_objects() {
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        assert!(mirrors(&mirrored));
        // The +X face lands on the -X side, facing that way.
        assert!(close(turned(mirrored, Vec3::X), Vec3::NEG_X));
        assert!(close(turned(mirrored, Vec3::Y), Vec3::Y));

        // A plane across Y keeps its area, and its normal, when Y is flattened.
        let flat = Mat4::from_rotation_x(1.0) * Mat4::from_scale(Vec3::new(2.0, 0.0, 3.0));
        assert!(!mirrors(&flat));
        let up = Mat4::from_rotation_x(1.0).transform_vector3(Vec3::Y);
        assert!(close(turned(flat, Vec3::Y), up));

        assert_eq!(
            normal_matrix(&Mat4::from_scale(Vec3::new(0.0, 0.0, 1.0))),
            None
        );
        assert_eq!(normal_matrix(&Mat4::from_translation(Vec3::NAN)), None);
    }

    #[test]
    fn normals_keep_their_direction_at_any_scale() {
        let normals = [Vec3::X, Vec3::Z, Vec3::new(1.0, -2.0, 0.5).normalize()];
        let rotated = |scale: Vec3| {
            Transform {
                position: Vec3::ZERO,
                rotation: Vec3::new(20.0, 30.0, 40.0),
                scale,
            }
            .matrix()
        };
        for scale in [1e-20, 1e20] {
            for normal in normals {
                let expected = turned(rotated(Vec3::ONE), normal);
                let got = turned(rotated(Vec3::splat(scale)), normal);
                assert!(close(got, expected), "{scale}: {normal} turned to {got}");
            }
        }
        let uneven = rotated(Vec3::new(1e-12, 1.0, 1e12));
        let inverse_transpose = Mat3::from_mat4(uneven).as_dmat3().inverse().transpose();
        for normal in normals {
            let expected = (inverse_transpose * normal.as_dvec3())
                .normalize()
                .as_vec3();
            let got = turned(uneven, normal);
            assert!(
                close(got, expected),
                "{normal} turned to {got}, not {expected}"
            );
        }
    }
}
