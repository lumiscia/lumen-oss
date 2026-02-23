# Quickstart: Lumen/Next Engine Development

**Feature**: 002-lumen-next-engine
**Date**: 2026-02-23

## Prerequisites

- Rust 1.75+ (2024 edition recommended)
- Skia build dependencies (see `skia-safe` docs for platform-specific requirements)
- For ffmpeg feature: `ffmpeg` libraries installed (`brew install ffmpeg` on macOS)

## Build

```bash
# Core crate only (no optional features)
cargo build -p lumen

# With JSON delegate
cargo build -p lumen --features json

# With FFmpeg adapter
cargo build -p lumen --features ffmpeg

# All features
cargo build -p lumen --features "json,ffmpeg,threading"
```

## Test

```bash
# Unit tests
cargo test -p lumen

# With JSON delegate tests
cargo test -p lumen --features json

# All tests
cargo test -p lumen --all-features
```

## Usage Example (Library Consumer)

```rust
use lumen::{
    Composition, Graph, Node, NodeKind, NodeId, Connection,
    TimelineSettings, RenderSettings, RenderContext, SurfacePool,
    RuntimeCapabilityProfile,
    node::{solid_color::SolidColor, media_output::MediaOutput},
};

// 1. Build a graph
let mut graph = Graph::new();
let solid = graph.add_node(Node::new(NodeKind::SolidColor(SolidColor {
    color: [255, 0, 0, 255],
    width: None,  // defaults to composition width
    height: None,
})));
let output = graph.add_node(Node::new(NodeKind::MediaOutput(MediaOutput)));
graph.connect(Connection {
    from_node: solid,
    from_port: OutputPort::default(),
    to_node: output,
    to_port: InputPort::named("source"),
}).unwrap();

// 2. Create composition
let composition = Composition::new(
    graph,
    TimelineSettings { fps: 30.0, duration_frames: 90 },
    RenderSettings { width: 1920, height: 1080, background_color: [0, 0, 0, 255] },
);

// 3. Validate
let profile = RuntimeCapabilityProfile::cpu_only();
composition.validate(&profile).unwrap();

// 4. Render a frame — method on Composition, not a free function
let pool = SurfacePool::new();
let mut ctx = RenderContext::new(&composition, &pool, &profile);
let frame = composition.render_frame(0, &mut ctx).unwrap();
let pixels: &[u8] = frame.as_bitmap_bytes();
```

## Module Map

```
lib.rs              → Public API re-exports
composition.rs      → Composition, TimelineSettings, RenderSettings
node.rs + node/     → Node, NodeId, NodeKind, per-node implementations
graph.rs            → Graph, Connection, validation, topological sort
render.rs           → render_frame, graph traversal, frame evaluation
raster.rs           → RasterFrame (Bitmap | Surface)
surface_pool.rs     → SurfacePool, SurfaceRef (RAII)
animation.rs        → KeyframeTrack, Keyframe, interpolation, sampling
expr.rs + expr/     → Expression AST, evaluator, builtins
media.rs            → MediaStore, ImageResolver, VideoFrameResolver traits
cache.rs            → AssetCache, NodeOutputCache, MemoCache
capability.rs       → RuntimeCapabilityProfile
error.rs            → LumenError, all error enums
sink.rs             → Sink trait
threading.rs        → RenderOrchestrator (feature-gated)
json.rs + json/     → JSON delegate (feature-gated)
ffmpeg.rs           → FFmpeg adapter (feature-gated)
```

## Key Patterns

- **Methods on types, not free functions**: `composition.render_frame()`, `Composition::from_json()`, `graph.validate()`
- **Trait + enum dispatch**: `NodeEval` trait enforces the contract per-struct; `NodeKind` enum dispatches via exhaustive match. Each node is its own struct: `NodeKind::Blur(blur::Blur)`
- **Newtype IDs**: `NodeId(u64)`, `TrackId(u64)` — never raw integers in APIs
- **Move semantics**: `RasterFrame` is moved through graph evaluation, cloned via `Arc` for fan-out
- **RAII surfaces**: `SurfaceRef` auto-returns to pool on drop
- **Feature gates**: `json`, `ffmpeg`, `threading` are opt-in via Cargo features
- **No `unwrap()` in library code**: All errors are `Result<T, LumenError>`
