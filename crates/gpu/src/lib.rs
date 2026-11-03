//! Operation-agnostic GPU render-plan core for Lumen.
//!
//! This crate deliberately does not model semantic nodes such as blur,
//! transform, text, or composite. Lumen owns those concepts and compiles them
//! into resources, shader programs, and generic passes that this crate can
//! allocate and execute.

mod binding;
mod geometry;
mod id;
#[cfg(all(target_os = "macos", feature = "metal"))]
mod metal;
mod pass;
mod plan;
mod program;
mod renderer;
mod resource;
mod update;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
mod vulkan_export;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
mod vulkan_video;

pub use binding::*;
pub use geometry::*;
pub use id::*;
#[cfg(all(target_os = "macos", feature = "metal"))]
pub use metal::*;
pub use pass::*;
pub use plan::*;
pub use program::*;
pub use renderer::*;
pub use resource::*;
pub use update::*;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
pub use vulkan_export::*;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
pub use vulkan_video::*;
pub use wgpu;
