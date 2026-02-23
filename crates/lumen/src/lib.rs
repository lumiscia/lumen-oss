//! Lumen compositing engine crate.

pub mod animation;
pub mod cache;
pub mod capability;
pub mod composition;
pub mod error;
pub mod expr;
pub mod graph;
pub mod media;
pub mod node;
pub mod raster;
pub mod render;
pub mod sink;
pub mod surface_pool;

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;

#[cfg(feature = "json")]
pub mod json;

#[cfg(feature = "threading")]
pub mod threading;

pub use animation::{AnimatableType, Extrapolation, InterpolationMode, Keyframe, KeyframeTrack};
pub use cache::{AssetCache, NodeOutputCache, SharedAssetCache, VideoMetadata};
pub use capability::{RuntimeCapabilityProfile, SinkType};
pub use composition::{Composition, CompositionMetadata, RenderSettings, TimelineSettings};
pub use error::{LumenError, Warning};
pub use expr::{ExprNode, Expression, ExpressionId, ExpressionValue};
pub use graph::{Connection, Graph, InputPort, OutputPort};
pub use media::{ImageResolver, MediaStore, VideoFrameResolver};
pub use node::{
    BlendMode, InputPortDef, Node, NodeEval, NodeId, NodeInputs, NodeKind, OutputPortDef, PortKind,
    PortValue, PropertyValue, TrackId, VectorData,
};
pub use raster::RasterFrame;
pub use render::{CancellationToken, NullMediaStore, RenderContext};
pub use sink::Sink;
pub use surface_pool::{SurfacePool, SurfaceRef};

#[cfg(feature = "json")]
pub use json::{JsonDelegate, JsonDelegateResult};

#[cfg(feature = "ffmpeg")]
pub use ffmpeg::{FfmpegMediaStore, FfmpegVideoResolver};

#[cfg(feature = "threading")]
pub use threading::{RenderOrchestrator, RenderWorkerPool};
