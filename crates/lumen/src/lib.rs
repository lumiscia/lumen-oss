//! Lumen compositing engine crate.

extern crate self as lumen;

use crate::error::LumenError;

#[cfg(all(feature = "ffmpeg", target_arch = "wasm32", target_os = "unknown"))]
compile_error!("Lumen's ffmpeg feature is native-only; use browser media APIs on wasm.");

pub mod audio;
pub mod composition;
pub mod error;
pub mod expr;
pub mod gpu;
pub mod graph;
pub mod media;
pub mod node;

#[cfg(feature = "ffmpeg")]
pub use media::ffmpeg;

#[cfg(feature = "image")]
pub use media::image;

#[cfg(feature = "json")]
pub mod json;

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
