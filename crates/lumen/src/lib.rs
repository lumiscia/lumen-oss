//! Lumen compositing engine crate.

use crate::error::LumenError;

pub mod composition;
pub mod error;
pub mod expr;
pub mod graph;
pub mod media;
pub mod node;
pub mod raster;
pub mod render;

mod backend;

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;

#[cfg(feature = "image")]
pub mod image;

#[cfg(feature = "json")]
pub mod json;

#[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
pub use backend::webgl::install_webgl_context;
#[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
pub use backend::webgl::present_webgl_image;

pub(crate) type Result<T> = std::result::Result<T, LumenError>;
