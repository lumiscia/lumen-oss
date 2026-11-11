//! Lumen compositing engine crate.
//!
//! A host parses a [`composition::Composition`] with [`json::parse`] (or builds one with
//! [`composition::CompositionBuilder`]), gives its media through a [`media::MediaStore`], and
//! renders its frames with [`gpu::GpuCompositionRenderer::render`]. Everything fallible returns
//! [`error::LumenError`]; warnings are [`diagnostic::Diagnostic`]s.
//!
//! - [`composition`], [`graph`], [`node`]: the composition model, its node graph and node kinds.
//! - [`json`]: parsing and checking composition JSON.
//! - [`expr`]: the expression language params can be written in.
//! - [`media`]: the resolver traits a host implements, and the resolvers the `ffmpeg` and `image`
//!   features add.
//! - [`gpu`]: the renderer and the targets frames render into.
//! - [`audio`]: audio timelines and their mix.
//! - [`diagnostic`], [`error`]: what checks and renders report.

extern crate self as lumen_engine;

use crate::error::LumenError;

#[cfg(all(feature = "ffmpeg", target_arch = "wasm32", target_os = "unknown"))]
compile_error!("Lumen's ffmpeg feature is native-only; use browser media APIs on wasm.");

pub mod audio;
pub mod composition;
pub mod diagnostic;
pub mod error;
pub mod expr;
pub(crate) mod fingerprint;
pub mod gpu;
pub mod graph;
pub mod json;
pub mod media;
pub mod node;
pub(crate) mod scope;

pub use json::SCHEMA_VERSION;

pub(crate) type Result<T> = std::result::Result<T, LumenError>;
