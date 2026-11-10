use glam::{Mat4, Vec3};

use crate::{Camera, Light};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blending {
    /// Hides what is behind it.
    Opaque,
    /// Shows what is behind it through its alpha, drawn after opaque objects.
    Blend,
}

/// How an object's surface is shaded, in linear light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    /// Linear colour with straight alpha.
    pub base_color: [f32; 4],
    /// 0 is a dielectric, 1 a bare metal.
    pub metallic: f32,
    /// 0 is a mirror finish, 1 fully diffuse.
    pub roughness: f32,
    /// Linear light the surface gives off on its own.
    pub emissive: [f32; 3],
    pub blending: Blending,
    /// Shows the base colour as is, ignoring lights.
    pub unlit: bool,
    /// Draws the back of each face too.
    pub double_sided: bool,
}

/// One mesh drawn somewhere in a scene. `M` names the mesh to whoever draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Object<M> {
    /// Mesh to world.
    pub world: Mat4,
    pub mesh: M,
    /// Indices the mesh draws, three to a triangle.
    pub index_count: u32,
    pub surface: Surface,
    /// The point blended objects sort by, in world space.
    pub center: Vec3,
}

/// What a scene holds at one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene<M> {
    pub objects: Vec<Object<M>>,
    pub lights: Vec<Light>,
    /// The first camera found, if any.
    pub camera: Option<Camera>,
}

impl<M> Default for Scene<M> {
    fn default() -> Self {
        Self {
            objects: Vec::new(),
            lights: Vec::new(),
            camera: None,
        }
    }
}
