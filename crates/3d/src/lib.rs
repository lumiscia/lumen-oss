//! Scene maths and mesh data for Lumen's 3D nodes, with nothing tied to a GPU: primitive meshes,
//! glTF models, transforms, cameras, lights, and the order and batches a frame's objects draw in.
//!
//! World units are metres, Y is up and cameras look down -Z.

mod camera;
mod color;
mod draws;
mod light;
mod mesh;
pub mod model;
mod scene;
mod transform;

pub use camera::{Camera, Projection};
pub use color::srgb_to_linear;
pub use draws::{DrawError, DrawKey, Draws, Row, Run};
pub use glam;
pub use light::{Light, LightKind, Lighting};
pub use mesh::{Mesh, Primitive, SkinVertex, Vertex, plane_scale};
pub use scene::{
    Blending, Filter, MAX_SCENE_NODES, Map, Maps, Object, Sampler, Scene, Surface, Wrap,
};
pub use transform::{Transform, look_at, mirrors, normal_matrix, rotation};
