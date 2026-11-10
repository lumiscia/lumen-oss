//! Scene maths and mesh data for Lumen's 3D nodes, with nothing tied to a GPU: primitive meshes,
//! transforms, cameras, lights, and the order and batches a frame's objects draw in.
//!
//! World units are metres, Y is up and cameras look down -Z.

mod camera;
mod color;
mod draws;
mod light;
mod mesh;
mod scene;
mod transform;

pub use camera::{Camera, Projection};
pub use color::srgb_to_linear;
pub use draws::{DrawError, DrawKey, Draws, MAX_OBJECTS, Row, Run};
pub use glam;
pub use light::{Light, LightKind, Lighting};
pub use mesh::{Mesh, Primitive, Vertex};
pub use scene::{Blending, Object, Scene, Surface};
pub use transform::{Transform, look_at, rotation};
