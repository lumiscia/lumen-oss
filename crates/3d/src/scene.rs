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
pub struct Surface<T> {
    /// Linear colour with straight alpha, which the texture is multiplied by.
    pub base_color: [f32; 4],
    /// A Lumen raster, premultiplied and sRGB-encoded, whose texels colour the surface through
    /// the mesh's texture coordinates.
    pub texture: Option<T>,
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

/// The most nodes one frame's scene may expand to, counting a node each time it is reached, which
/// also bounds the objects it draws.
pub const MAX_SCENE_NODES: usize = 100_000;

/// One mesh drawn somewhere in a scene. `M` names the mesh and `T` the texture to whoever draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Object<M, T> {
    /// Mesh to world.
    pub world: Mat4,
    pub mesh: M,
    /// Indices the mesh draws, three to a triangle.
    pub index_count: u32,
    pub surface: Surface<T>,
    /// The point blended objects sort by, in world space.
    pub center: Vec3,
}

/// What a scene holds at one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene<M, T> {
    pub objects: Vec<Object<M, T>>,
    pub lights: Vec<Light>,
    /// The first camera found, if any.
    pub camera: Option<Camera>,
}

impl<M, T> Default for Scene<M, T> {
    fn default() -> Self {
        Self {
            objects: Vec::new(),
            lights: Vec::new(),
            camera: None,
        }
    }
}
