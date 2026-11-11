//! Lumen compositing engine crate.

extern crate self as lumen_engine;

use crate::error::LumenError;

#[cfg(all(feature = "ffmpeg", target_arch = "wasm32", target_os = "unknown"))]
compile_error!("Lumen's ffmpeg feature is native-only; use browser media APIs on wasm.");

pub mod audio;
pub mod composition;
pub mod diagnostic;
pub mod error;
pub mod expr;
#[doc(hidden)]
pub mod fingerprint;
pub mod gpu;
pub mod graph;
pub mod media;
pub mod node;
pub mod scope;

#[cfg(feature = "ffmpeg")]
pub use media::ffmpeg;

#[cfg(feature = "image")]
pub use media::image;

pub mod json;
pub use json::SCHEMA_VERSION;

pub(crate) type Result<T> = std::result::Result<T, LumenError>;
