//! Lumen compositing engine crate.

use crate::error::LumenError;

#[cfg(all(feature = "ffmpeg", target_arch = "wasm32", target_os = "unknown"))]
compile_error!("Lumen's ffmpeg feature is native-only; use browser media APIs on wasm.");

#[cfg(all(feature = "ffmpeg", not(any(feature = "metal", feature = "vulkan"))))]
compile_error!(
    "Lumen's ffmpeg feature requires a GPU backend feature: enable `metal` or `vulkan`."
);

pub mod audio;
pub mod composition;
pub mod error;
pub mod expr;
pub mod gpu_image;
pub mod graph;
pub mod media;
pub mod node;
pub mod render;

mod backend;

#[cfg(feature = "ffmpeg")]
pub use media::ffmpeg;

#[cfg(feature = "image")]
pub use media::image;

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
#[allow(dead_code)]
pub(crate) fn debug_log(_message: &str) {}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[allow(dead_code)]
pub(crate) fn debug_log(message: &str) {
    eprintln!("{message}");
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[allow(dead_code)]
pub(crate) fn debug_error(_message: &str) {}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[allow(dead_code)]
pub(crate) fn debug_error(message: &str) {
    eprintln!("{message}");
}
