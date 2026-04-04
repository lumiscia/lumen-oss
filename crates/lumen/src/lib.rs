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
pub mod sink;

mod backend;

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;

#[cfg(feature = "image")]
pub mod image;

#[cfg(feature = "json")]
pub mod json;

pub(crate) type Result<T> = std::result::Result<T, LumenError>;
