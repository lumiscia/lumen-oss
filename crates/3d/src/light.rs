use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKind {
    /// Parallel rays along the light's -Z, like the sun.
    Directional,
    /// Shines every way from its position.
    Point,
    /// A cone along the light's -Z from its position.
    Spot,
    /// Lights every surface evenly from all around.
    Ambient,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    /// Linear colour times intensity.
    pub radiance: Vec3,
    pub position: Vec3,
    /// The direction the light travels, unit length.
    pub direction: Vec3,
    /// Distance past which a point or spot light has no effect; 0 is unlimited.
    pub range: f32,
    /// Cosines of the angles from a spot light's axis where its cone ends and where it starts
    /// fading out.
    pub cos_outer: f32,
    pub cos_inner: f32,
    /// How the light casts shadows, if it does.
    pub shadow: Option<Shadow>,
}

/// How a light's shadows look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    /// Width in world units over which shadow edges blur, within the limits of the light's shadow
    /// map.
    pub softness: f32,
    /// How far from a directional light's camera along its view its shadows reach, fading out
    /// over the last tenth.
    pub distance: f32,
}

impl Light {
    /// A light placed by `world`, shining along its -Z. A spot light's cone is `spot_angle`
    /// degrees across, and fades out over the `spot_softness` fraction of it nearest its edge. It
    /// casts no shadows.
    pub fn new(
        kind: LightKind,
        radiance: Vec3,
        world: Mat4,
        range: f32,
        spot_angle: f32,
        spot_softness: f32,
    ) -> Self {
        let half = (spot_angle.clamp(0.0, 179.0) * 0.5).to_radians();
        Self {
            kind,
            radiance: radiance.max(Vec3::ZERO),
            position: world.transform_point3(Vec3::ZERO),
            direction: world
                .transform_vector3(Vec3::NEG_Z)
                .normalize_or(Vec3::NEG_Z),
            range: range.max(0.0),
            cos_outer: half.cos(),
            cos_inner: (half * (1.0 - spot_softness.clamp(0.0, 1.0))).cos(),
            shadow: None,
        }
    }
}

/// How a scene is lit: the lights that shine from somewhere, and the ambient light from all
/// around.
#[derive(Debug, Clone, PartialEq)]
pub struct Lighting {
    /// Every light but the ambient ones.
    pub direct: Vec<Light>,
    pub ambient: Vec3,
}

impl Lighting {
    /// `lights` split into direct light, in their order, and ambient light. A scene without any lights gets a key light
    /// from above, right and in front, and a dim ambient fill.
    pub fn of(lights: &[Light]) -> Self {
        if lights.is_empty() {
            let key = Light {
                kind: LightKind::Directional,
                radiance: Vec3::splat(3.0),
                position: Vec3::ZERO,
                direction: Vec3::new(0.4, -0.8, -0.45).normalize(),
                range: 0.0,
                cos_outer: 0.0,
                cos_inner: 0.0,
                shadow: None,
            };
            return Self {
                direct: vec![key],
                ambient: Vec3::splat(0.2),
            };
        }
        let (ambient, direct): (Vec<Light>, Vec<Light>) = lights
            .iter()
            .partition(|light| light.kind == LightKind::Ambient);
        Self {
            direct,
            ambient: ambient.iter().map(|light| light.radiance).sum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_light_shines_down_its_minus_z_from_its_position() {
        let world = Mat4::from_rotation_translation(
            crate::rotation(Vec3::new(-90.0, 0.0, 0.0)),
            Vec3::new(0.0, 3.0, 0.0),
        );
        let spot = Light::new(LightKind::Spot, Vec3::ONE, world, 0.0, 90.0, 0.5);
        assert_eq!(spot.position, Vec3::new(0.0, 3.0, 0.0));
        assert!((spot.direction - Vec3::NEG_Y).length() < 1e-6);
        assert!((spot.cos_outer - 45f32.to_radians().cos()).abs() < 1e-6);
        assert!((spot.cos_inner - 22.5f32.to_radians().cos()).abs() < 1e-6);
    }

    #[test]
    fn only_a_scene_without_lights_gets_the_default_ones() {
        let defaults = Lighting::of(&[]);
        assert_eq!(defaults.direct.len(), 1);
        assert_eq!(defaults.ambient, Vec3::splat(0.2));

        let ambient = Light::new(
            LightKind::Ambient,
            Vec3::splat(0.5),
            Mat4::IDENTITY,
            0.0,
            0.0,
            0.0,
        );
        let only_ambient = Lighting::of(&[ambient, ambient]);
        assert!(only_ambient.direct.is_empty());
        assert_eq!(only_ambient.ambient, Vec3::ONE);
    }
}
