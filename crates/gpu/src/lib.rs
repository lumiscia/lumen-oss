//! Operation-agnostic GPU recording core for Lumen.
//!
//! This crate deliberately does not model semantic nodes such as blur,
//! transform, text, or composite. Lumen owns those concepts and records them as
//! generic passes over the textures, buffers and shader programs this crate
//! allocates and caches.

mod geometry;
#[cfg(all(target_os = "macos", feature = "metal"))]
mod metal;
mod recorder;
mod renderer;
mod resource;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
mod vulkan_export;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
mod vulkan_video;

pub use geometry::*;
#[cfg(all(target_os = "macos", feature = "metal"))]
pub use metal::*;
pub use recorder::*;
pub use renderer::*;
pub use resource::*;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
pub use vulkan_export::*;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
pub use vulkan_video::*;
pub use wgpu;
