//! Lumen compositing engine crate.

use crate::error::LumenError;

pub mod audio;
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
pub use backend::webgl::image_frame_from_video_frame;
#[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
pub use backend::webgl::install_webgl_context;
#[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
pub use backend::webgl::present_webgl_image;
#[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
pub use backend::webgl::with_webgl_surface_context;

pub(crate) type Result<T> = std::result::Result<T, LumenError>;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn debug_log(_message: &str) {}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) fn debug_log(message: &str) {
    eprintln!("{message}");
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn debug_error(_message: &str) {}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) fn debug_error(message: &str) {
    eprintln!("{message}");
}
