use glam::{Mat4, Vec3};

/// An axis-aligned box, empty when `min` passes `max` on any axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    pub const EMPTY: Self = Self {
        min: Vec3::INFINITY,
        max: Vec3::NEG_INFINITY,
    };

    /// The box around `points`, leaving out those that aren't finite.
    pub fn of(points: impl IntoIterator<Item = Vec3>) -> Self {
        points
            .into_iter()
            .filter(|point| point.is_finite())
            .fold(Self::EMPTY, |bounds, point| Self {
                min: bounds.min.min(point),
                max: bounds.max.max(point),
            })
    }

    pub fn is_empty(&self) -> bool {
        !self.min.cmple(self.max).all()
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn corners(&self) -> [Vec3; 8] {
        let (min, max) = (self.min, self.max);
        [0, 1, 2, 3, 4, 5, 6, 7].map(|corner| {
            Vec3::select(
                glam::BVec3::new(corner & 1 != 0, corner & 2 != 0, corner & 4 != 0),
                max,
                min,
            )
        })
    }

    /// The box around this one moved by `matrix`: empty when this one is, or when that leaves
    /// any corner not finite.
    pub fn transformed(&self, matrix: &Mat4) -> Self {
        if self.is_empty() {
            return Self::EMPTY;
        }
        let corners = self.corners().map(|corner| matrix.transform_point3(corner));
        if corners.iter().any(|corner| !corner.is_finite()) {
            return Self::EMPTY;
        }
        Self::of(corners)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_moves_with_its_corners_and_stays_empty_once_not_finite() {
        let unit = Bounds::of([Vec3::splat(-0.5), Vec3::splat(0.5)]);
        let turned = unit.transformed(&Mat4::from_rotation_y(45f32.to_radians()));
        let half_diagonal = 0.5 * 2f32.sqrt();
        assert!((turned.max.x - half_diagonal).abs() < 1e-6);
        assert!((turned.max.y - 0.5).abs() < 1e-6);
        assert!(
            unit.transformed(&Mat4::from_translation(Vec3::NAN))
                .is_empty()
        );
        assert!(Bounds::EMPTY.transformed(&Mat4::IDENTITY).is_empty());
        assert!(Bounds::of([Vec3::INFINITY]).is_empty());
    }
}
