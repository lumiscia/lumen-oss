# Public API Contract: crates/lumen

**Feature**: 002-lumen-next-engine
**Date**: 2026-02-23

This document defines the public API surface of the `lumen` crate — the contract consumed by `lumen-wasm`, `lumen-local`, and `lumen-server`.

---

## Crate Root Exports (`lib.rs`)

```rust
// Core types
pub use composition::{Composition, TimelineSettings, RenderSettings};
pub use node::{Node, NodeId, NodeKind, NodeEval, NodeInputs, PortValue, PortKind};
pub use node::{InputPortDef, OutputPortDef};
pub use graph::{Graph, Connection, InputPort, OutputPort};
pub use raster::RasterFrame;
pub use surface_pool::{SurfacePool, SurfaceRef};
pub use animation::{KeyframeTrack, TrackId, Keyframe, InterpolationMode, Extrapolation};
pub use expr::{Expression, ExpressionId, ExprNode, ExpressionValue};
pub use media::{MediaStore, ImageResolver, VideoFrameResolver};
pub use render::RenderContext;
pub use capability::RuntimeCapabilityProfile;
pub use error::LumenError;
pub use sink::Sink;

// Feature-gated
#[cfg(feature = "json")]
pub use json::{JsonDelegate, JsonDelegateResult};

#[cfg(feature = "ffmpeg")]
pub use ffmpeg::{FfmpegVideoResolver, FfmpegMediaStore};

#[cfg(feature = "threading")]
pub use threading::{RenderOrchestrator, RenderWorkerPool};
```

---

## Core API Operations

### Composition Construction

```rust
impl Composition {
    /// Build a composition from a graph, timeline, and render settings.
    pub fn new(graph: Graph, timeline: TimelineSettings, render: RenderSettings) -> Self;

    /// Add a keyframe track.
    pub fn add_track(&mut self, track: KeyframeTrack);

    /// Validate the composition against a runtime capability profile.
    /// Returns all validation errors/warnings.
    pub fn validate(&self, profile: &RuntimeCapabilityProfile) -> Result<Vec<Warning>, Vec<LumenError>>;
}
```

### Graph Construction

```rust
impl Graph {
    pub fn new() -> Self;
    pub fn add_node(&mut self, node: Node) -> NodeId;
    pub fn connect(&mut self, connection: Connection) -> Result<(), LumenError>;
    pub fn remove_node(&mut self, id: NodeId) -> Result<Node, LumenError>;
    pub fn remove_connection(&mut self, from: (NodeId, OutputPort), to: (NodeId, InputPort)) -> Result<(), LumenError>;

    /// Validate graph structure (cycles, ports, required inputs).
    pub fn validate(&self) -> Result<Vec<Warning>, Vec<LumenError>>;

    /// Topological sort from a target node (MediaOutput).
    pub fn evaluation_order(&self, target: NodeId) -> Result<Vec<NodeId>, LumenError>;
}
```

### Single-Frame Render

```rust
impl Composition {
    /// Render a single frame of a validated composition.
    /// Returns the output RasterFrame (bitmap-backed at sink boundary).
    pub fn render_frame(
        &self,
        frame: u32,
        context: &mut RenderContext,
    ) -> Result<RasterFrame, LumenError>;
}
```

### Multi-Frame Render (feature = "threading")

```rust
impl Composition {
    /// Render a range of frames using worker threads.
    /// Frames are submitted to the sink in order.
    pub fn render_sequence(
        &self,
        frame_range: Range<u32>,
        context: RenderContext,  // moved, cloned per-worker internally
        sink: Box<dyn Sink>,
        worker_count: usize,
    ) -> Result<(), LumenError>;
}
```

### Node Evaluation Contract

```rust
/// The contract every node struct must implement.
/// Not used for dyn dispatch — exists for compile-time enforcement.
pub trait NodeEval {
    /// Static port descriptors for graph validation and editor UI.
    fn input_port_defs(&self) -> &'static [InputPortDef];
    fn output_port_defs(&self) -> &'static [OutputPortDef];

    /// Evaluate the node. All connected inputs are pre-populated in `inputs`.
    fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError>;
}

/// NodeKind enum dispatches to each struct's NodeEval impl.
/// Each variant wraps a concrete struct: NodeKind::Blur(blur::Blur), etc.
impl NodeKind {
    pub fn evaluate(&self, inputs: &NodeInputs, ctx: &mut RenderContext) -> Result<PortValue, LumenError>;
    pub fn input_port_defs(&self) -> &'static [InputPortDef];
    pub fn output_port_defs(&self) -> &'static [OutputPortDef];
}
```

### Keyframe Operations

```rust
impl KeyframeTrack {
    pub fn new(id: TrackId, node_id: NodeId, property_path: PropertyPath, value_type: AnimatableType) -> Self;
    pub fn set_key(&mut self, frame: u32, value: PropertyValue, interpolation: InterpolationMode);
    pub fn remove_key(&mut self, frame: u32) -> Option<Keyframe>;
    pub fn sample(&self, frame: u32) -> PropertyValue;
}

impl Composition {
    /// Sample a property value at a given frame, respecting precedence:
    /// expression > keyframe > static literal.
    pub fn sample_property(
        &self,
        node_id: NodeId,
        property_path: &PropertyPath,
        frame: u32,
        context: &RenderContext,
    ) -> Result<PropertyValue, LumenError>;
}
```

### JSON Delegate (feature = "json")

```rust
/// Construct a Composition from JSON input.
/// Validates schema, parses expressions, and returns structured diagnostics.
impl Composition {
    pub fn from_json(input: &str) -> JsonDelegateResult;
}

/// Also available via TryFrom for ergonomic use:
impl TryFrom<&str> for Composition { /* delegates to from_json */ }

pub struct JsonDelegateResult {
    pub status: JsonDelegateStatus,
    pub composition: Option<Composition>,
    pub errors: Vec<LumenError>,
    pub warnings: Vec<Warning>,
}
```

### Media Traits

```rust
pub trait MediaStore: Send + Sync {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>>;
    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoResolver>>;
}

pub trait ImageResolver: Send + Sync {
    fn id(&self) -> &str;
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn resolve(&self) -> Result<Vec<u8>, MediaError>;
}

pub trait VideoFrameResolver: Send + Sync {
    fn id(&self) -> &str;
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn frame_count(&self) -> u32;
    fn resolve_frame(&self, frame: u32) -> Result<Vec<u8>, MediaError>;
}
```

### Sink Trait

```rust
pub trait Sink: Send {
    /// Accept a rendered frame. Frames arrive in order.
    fn write_frame(&mut self, frame: u32, data: &RasterFrame) -> Result<(), SinkError>;

    /// Signal that all frames have been submitted.
    fn finalize(&mut self) -> Result<(), SinkError>;
}
```

---

## Feature Flags

| Feature | Enables | Dependencies |
|---|---|---|
| `json` | JSON delegate, `lumen_graph_v1` schema parsing | serde, serde_json |
| `ffmpeg` | FFmpeg-backed video/image resolvers | ffmpeg-next |
| `threading` | Multi-frame parallel render orchestrator | crossbeam-channel, parking_lot |

---

## Breaking Changes from Legacy API

| Legacy | New | Migration |
|---|---|---|
| `Scene` / `Project` | `Composition` | Full replacement |
| `Layer` + `Vec<ClipType>` | `Graph` + `Vec<Node>` + `Vec<Connection>` | Full replacement |
| `render_scene(scene, frame, ctx)` | `composition.render_frame(frame, ctx)` | Method on Composition |
| `RendererContext` | `RenderContext` | New fields, new construction |
| `StyleProperty<T>` / `Sequence<T>` | `KeyframeTrack` + `PropertyValue` | Full replacement |
| `StyleExpression<T>` (string-based) | `Expression` (pre-parsed AST) | Full replacement |
| `convert_json_delegate(req)` | `Composition::from_json(str)` | Method on Composition |
| `chat_story_v1` JSON schema | `lumen_graph_v1` JSON schema | Full replacement |
| `RenderBackend` trait (CPU/GPU) | Direct CPU render path | Removed |
| `StreamingAssets` | `AssetCache` + `MediaStore` trait | Full replacement |
