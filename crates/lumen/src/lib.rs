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

#[cfg(feature = "json")]
pub mod json;

#[cfg(not(feature = "threading"))]
pub(crate) type SharedPointer<T> = std::rc::Rc<T>;

#[cfg(feature = "threading")]
pub(crate) type SharedPointer<T> = std::sync::Arc<T>;

pub(crate) type Result<T> = std::result::Result<T, LumenError>;
