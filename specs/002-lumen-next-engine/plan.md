# Implementation Plan: Lumen/Next Compositing Engine

**Branch**: `002-lumen-next-engine` | **Date**: 2026-02-23 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/002-lumen-next-engine/spec.md`

## Summary

Replace the existing layer/clip-based compositor in `crates/lumen` with a Fusion-style node graph engine for 2D compositing and motion graphics. This is a **full migration** — all legacy code (clip system, layer model, `StyleProperty<T>`, `chat_story_v1` schema, GPU backend stubs) is removed with no deprecation or fallback paths. The new engine uses CPU-first Skia rendering with pooled surfaces (`RasterFrame` union type), typed expression AST evaluation, keyframe animation tracks, trait-driven media integration, and frame-parallel multithreading on native platforms.

**Migration scope**: `crates/lumen` is fully rewritten. Dependent crates (`lumen-wasm`, `lumen-local`, `lumen-server`) must update their integration code to the new public API.

## Technical Context

**Language/Version**: Rust 1.75+ (2024 edition)
**Primary Dependencies**: `skia-safe` 0.91 (textlayout), `thiserror` 2.x, `parking_lot`, `crossbeam-channel`, `smallvec`
**Optional Dependencies**: `ffmpeg-next` (feature: ffmpeg), `serde` + `serde_json` (feature: json)
**Storage**: N/A (in-memory caches, no persistent storage in v1)
**Testing**: `cargo test`, property-based tests via `proptest`, snapshot/reference image tests
**Target Platform**: Native (macOS, Linux) + wasm (single-threaded)
**Project Type**: Library crate consumed by `lumen-wasm`, `lumen-local`, `lumen-server`
**Performance Goals**: Deterministic frame rendering; near-linear multithreaded scaling (4 workers ≥ 3x throughput); sub-frame render latency for interactive preview (target: 1080p in <50ms for simple graphs)
**Constraints**: CPU-only in v1 (no GPU); RGBA8 premultiplied sRGB fixed format; no audio; no bezier keyframes
**Scale/Scope**: Compositions up to 50 nodes; frame sequences up to thousands of frames for export

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- [x] All external inputs and trust boundaries are identified, with explicit validation strategy.
  - **Boundaries**: Media file I/O (image/video sources via `MediaStore` trait), JSON deserialization input (schema-validated), platform trait adapters, sink output. All validated at composition validation time before rendering.
- [x] Contract and schema changes are mapped to all impacted consumers.
  - **Consumers**: `lumen-wasm`, `lumen-local`, `lumen-server` — all must update to new `Composition` + `Graph` + `render_frame` API. JSON schema changes from `chat_story_v1` → `lumen_graph_v1`. See [contracts/public-api.md](./contracts/public-api.md).
- [x] Security impact is reviewed (auth, secrets, data access, abuse/failure modes).
  - **Expression sandbox**: Expressions evaluate only built-in globals and math functions. No arbitrary code execution, no filesystem access, no network access.
  - **Media path validation**: Source paths validated during composition validation; no path traversal allowed.
  - **JSON deserialization**: Schema-validated with structured error rejection; no silent acceptance of malformed input.
- [x] Tests cover the changed behavior at the correct boundary level.
  - **Unit**: Per-module tests for graph validation, keyframe sampling, expression evaluation, surface pool.
  - **Integration**: Graph construction → render → pixel verification tests.
  - **Property-based**: Expression evaluator correctness, keyframe interpolation accuracy.
  - **Snapshot**: Deterministic render output for reference compositions.
- [x] Operational safeguards are defined (bounded queues/caches, observability, rollback path).
  - **Bounded**: `crossbeam_channel::bounded` for threading job/result queues. `SurfacePool` grows but surfaces are returned via RAII `SurfaceRef`. `NodeOutputCache` cleared per render session.
  - **Observability**: Structured `LumenError` with context fields. `RenderStageObserver` trait for render pipeline tracing.
  - **Rollback**: N/A for library crate — no persistent state to roll back. Cancellation token for aborting long renders.

## Project Structure

### Documentation (this feature)

```text
specs/002-lumen-next-engine/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0: research decisions
├── data-model.md        # Phase 1: entity definitions
├── quickstart.md        # Phase 1: development guide
├── contracts/
│   └── public-api.md    # Phase 1: public API contract
├── checklists/
│   └── requirements.md  # Spec quality checklist
└── tasks.md             # Phase 2: implementation tasks (created by /speckit.tasks)
```

### Source Code (repository root)

```text
crates/lumen/
├── Cargo.toml
├── ARCHITECTURE.md
├── src/
│   ├── lib.rs                  # Public API re-exports, crate docs
│   ├── composition.rs          # Composition, TimelineSettings, RenderSettings
│   ├── graph.rs                # Graph, Connection, InputPort, OutputPort, validation, topo sort
│   ├── node.rs                 # Node, NodeId, NodeKind enum, NodeProperties, port type defs
│   ├── node/                   # Per-node-kind evaluation implementations
│   │   ├── transform.rs        # Scale → Rotate → Translate, pivot, bilinear sampling
│   │   ├── merge.rs            # Base/overlay/mask compositing, blend modes, opacity
│   │   ├── media_in.rs         # Image/Video source, range mapping, speed, loop
│   │   ├── solid_color.rs      # Solid color fill at composition or custom dimensions
│   │   ├── text.rs             # Skia paragraph builder, font layout, alignment
│   │   ├── shape.rs            # Rectangle, Ellipse, Polygon vector geometry
│   │   ├── shape_renderer.rs   # Vector → RasterFrame rasterization with fill/stroke
│   │   ├── crop.rs             # Rectangular crop
│   │   ├── resize.rs           # Stretch/Fit/Fill with Nearest/Bilinear sampling
│   │   ├── blur.rs             # Gaussian blur via Skia
│   │   ├── shadow.rs           # Alpha-derived drop shadow
│   │   ├── boolean.rs          # Shape/raster mask with invert
│   │   ├── switch.rs           # Frame-range input selector
│   │   ├── frame_hold.rs       # Freeze at specified frame
│   │   ├── media_output.rs     # Composition output edge node
│   │   └── memo.rs             # Cross-session cache with eligibility analysis
│   ├── raster.rs               # RasterFrame enum (Bitmap | Surface), conversions
│   ├── surface_pool.rs         # SurfacePool, SurfaceRef (RAII drop → return to pool)
│   ├── render.rs               # render_frame, graph traversal, node evaluation dispatch
│   ├── animation.rs            # KeyframeTrack, TrackId, Keyframe, interpolation, sampling
│   ├── expr.rs                 # Expression module root
│   ├── expr/                   # Expression engine internals
│   │   ├── ast.rs              # ExprNode, ExpressionValue, BinaryOp, UnaryOp, BuiltinFn
│   │   ├── parser.rs           # Pratt parser, lexer (adapted from existing)
│   │   ├── eval.rs             # AST evaluator with RenderContext globals
│   │   └── builtins.rs         # Built-in function implementations
│   ├── media.rs                # MediaStore, ImageResolver, VideoFrameResolver traits
│   ├── cache.rs                # AssetCache, NodeOutputCache, MemoCache
│   ├── capability.rs           # RuntimeCapabilityProfile, validation against profile
│   ├── error.rs                # LumenError, GraphValidationError, RenderError, etc.
│   ├── sink.rs                 # Sink trait definition
│   ├── threading.rs            # RenderOrchestrator, worker pool (feature: threading)
│   ├── json.rs                 # JSON delegate root (feature: json)
│   ├── json/                   # JSON internals (feature: json)
│   │   ├── schema.rs           # lumen_graph_v1 serde types
│   │   └── convert.rs          # Schema → Composition conversion
│   └── ffmpeg.rs               # FFmpeg video resolver adapter (feature: ffmpeg)
└── tests/
    ├── graph_validation.rs     # Graph structure validation tests
    ├── render_basic.rs         # Single-frame render + pixel verification
    ├── animation.rs            # Keyframe sampling correctness
    ├── expressions.rs          # Expression evaluation tests
    └── threading.rs            # Multi-frame ordered output tests
```

**Structure Decision**: Rust 2018+ flat-module layout. `node.rs` defines the `NodeKind` enum and shared types; `node/` directory contains per-node evaluation implementations. Same pattern for `expr.rs` + `expr/` and `json.rs` + `json/`. No `mod.rs` files — use file-as-module convention.

### Files Deleted (Legacy Removal)

All legacy files are removed with no replacement stubs:

| Deleted Path | Reason |
|---|---|
| `src/scene.rs` | Replaced by `composition.rs` |
| `src/clip/` (entire directory) | Replaced by `node/` |
| `src/clip/style/` (entire directory) | Replaced by `animation.rs` + node properties |
| `src/dependency/` (entire directory) | Replaced by `graph.rs` topo sort |
| `src/render/backend/` (entire directory) | GPU stubs removed; unified CPU path in `render.rs` |
| `src/render/backend/software.rs` | Absorbed into `render.rs` |
| `src/json/enabled.rs` | `chat_story_v1` → `json/schema.rs` (`lumen_graph_v1`) |
| `src/time.rs` | `Rational` replaced by `f32` fps in `TimelineSettings` |

### Files Adapted (Not Deleted)

| File | Changes |
|---|---|
| `src/lib.rs` | Rewritten: new module declarations and public re-exports |
| `src/media.rs` | Traits retained, signatures updated for `Result<Vec<u8>, MediaError>` |
| `src/expr/` | Parser/evaluator core retained, scope/builtins extended |
| `src/render/context.rs` | Rewritten as part of `render.rs` with new `RenderContext` |
| `src/ffmpeg/` | Adapted to implement new `VideoFrameResolver` trait |
| `src/json/mod.rs` | Delegate framework retained, new schema (`lumen_graph_v1`) |
| `Cargo.toml` | Updated deps: add `parking_lot`, `crossbeam-channel`, `smallvec`; add `threading` feature |

## Rust Patterns and Conventions

### Type System

- **Newtype IDs**: `NodeId(u64)`, `TrackId(u64)` — derive `Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Display`. Use `#[serde(transparent)]` behind `json` feature.
- **Node architecture (trait + enum dispatch)**:
  - Each node kind is its own struct in its own file (e.g., `node/blur.rs` → `pub struct Blur { ... }`).
  - A `NodeEval` trait defines the contract every node must implement: `input_port_defs()`, `output_port_defs()`, `evaluate(&self, inputs: &NodeInputs, ctx: &mut RenderContext) -> Result<PortValue, LumenError>`.
  - `NodeKind` is a `#[non_exhaustive]` enum where each variant wraps its struct: `NodeKind::Blur(blur::Blur)`, `NodeKind::Merge(merge::Merge)`, etc.
  - `NodeKind` delegates to each struct's `NodeEval` impl via exhaustive `match` — no `dyn` dispatch, no vtable. Adding a new variant without wiring up the match is a compile error.
  - This gives two compile-time safety nets: the trait enforces the contract per-struct, the enum match enforces exhaustive dispatch.
- **Typed ports**: `PortKind` enum (`RasterFrame`, `Surface`, `Vector`). `InputPortDef` and `OutputPortDef` structs carry name + kind + optional flag. `NodeInputs` wraps a `HashMap<&'static str, PortValue>` with typed accessor methods (`get_raster()`, `get_raster_optional()`, `get_vector()`).
- **Property values**: `PropertyValue` enum (`Float(f64)`, `Int(i64)`, `Bool(bool)`, `Color([u8; 4])`, `String(String)`, `Vector2(f64, f64)`, `Map(HashMap<String, PropertyValue>)`).

### Ownership

- **`RasterFrame`**: Owned value moved through graph evaluation. `Bitmap` variant uses `Arc<Vec<u8>>` for cheap cloning on fan-out. `Surface` variant is unique (not cloneable — must convert to bitmap for fan-out).
- **`SurfaceRef`**: RAII guard. `impl Drop for SurfaceRef` clears and returns surface to pool.
- **`RenderContext`**: Passed as `&mut` to evaluation functions. Owns caches and pool references.
- **`Arc<T>`**: Used for shared immutable data across threads (composition graph, decoded image cache entries).

### Error Handling

- **`thiserror`** for all error types. Each category is its own enum with structured context fields.
- **No `unwrap()` or `expect()`** in library code. All fallible operations return `Result`.
- **`#[cold]`** on error construction paths to keep hot paths compact.
- **Structured context**: `GraphValidationError::Cycle { path: Vec<NodeId> }`, not `"cycle detected: ..."`.

### Performance

- **`SmallVec<[T; 4]>`** for node input/output port lists (most nodes have 1–3 ports).
- **`parking_lot::Mutex`/`RwLock`** for synchronization (faster uncontended than std).
- **`crossbeam_channel::bounded`** for threading MPSC channels.
- **`#[repr(u8)]`** on small enums (`BlendMode`, `PortType`, `InterpolationMode`).
- **`SurfacePool`** amortizes Skia surface allocation across frames.

### Testing

- **Unit tests**: `#[cfg(test)] mod tests` in each module file.
- **Integration tests**: `tests/` directory for graph → render → pixel verification.
- **Property-based**: `proptest` for expression evaluation and keyframe interpolation.
- **Snapshot**: Deterministic hash-based comparison for render output.
- **Mock media**: Test implementations of `MediaStore`, `ImageResolver`, `VideoFrameResolver`.

### API Design

- **Associated functions and `From`/`TryFrom` impls over free functions**. Prefer `Composition::from_json(str)` or `impl TryFrom<&str> for Composition` over standalone `convert_json(str)`. Similarly, `Graph::validate(&self)` not `validate_graph(&graph)`. This keeps behavior co-located with the type it operates on.
- **`pub`** only on intended API in `lib.rs`. Internal modules use `pub(crate)`.
- **Builder pattern** for `Composition` (many optional fields).
- **`impl From<X> for Y`** for lossless conversions (e.g., `BlendMode` → `skia_safe::BlendMode`).
- **No public `unsafe`** in v1.
- **Feature gates**: `#[cfg(feature = "json")]`, `#[cfg(feature = "ffmpeg")]`, `#[cfg(feature = "threading")]`.

## Implementation Phases

### Phase 1: Core Graph + Single-Frame Render (P1 — MVP)

Foundation that all other phases build on.

1. **Error types** (`error.rs`): Define `LumenError` and all error category enums with structured context fields.
2. **Core types** (`node.rs`, `composition.rs`, `graph.rs`): `NodeId`, `NodeKind` enum, `NodeEval` trait, `Node`, `Graph`, `Connection`, `NodeInputs`, `PortValue`, `InputPortDef`/`OutputPortDef`, `Composition`, `TimelineSettings`, `RenderSettings`. Graph validation (cycles, port types, required inputs, MediaOutput target).
3. **Raster pipeline** (`raster.rs`, `surface_pool.rs`): `RasterFrame` enum, `SurfacePool`, `SurfaceRef` RAII. Bitmap ↔ Surface promotion/conversion.
4. **Render engine** (`render.rs`): Graph traversal from MediaOutput via topological order. Node evaluation dispatch via `NodeKind::evaluate()` (exhaustive match delegating to each struct's `NodeEval::evaluate` impl). `RenderContext` construction. `NodeInputs` population from upstream `PortValue` outputs.
5. **Source nodes** (`node/solid_color.rs`, `node/shape.rs`, `node/shape_renderer.rs`, `node/text.rs`): Nodes that produce `RasterFrame` or `Vector` without upstream inputs.
6. **Transform + effect nodes** (`node/transform.rs`, `node/crop.rs`, `node/resize.rs`, `node/blur.rs`, `node/shadow.rs`): Single-input → single-output raster processing.
7. **Compositing** (`node/merge.rs`, `node/boolean.rs`): Multi-input compositing with blend modes, opacity, masks. Premultiplied alpha semantics.
8. **Utility nodes** (`node/switch.rs`, `node/frame_hold.rs`, `node/media_output.rs`): Switch frame-range selection, frame hold, output boundary.
9. **Sink** (`sink.rs`): `Sink` trait definition, basic `BitmapSink` for testing.
10. **Legacy removal**: Delete `scene.rs`, `clip/`, `dependency/`, `render/backend/`, `time.rs`. Rewrite `lib.rs`.
11. **Graph optimization** (`graph.rs` or `render.rs`): Dead node elimination, frame-range culling, fan-out memoization, no-op elimination.

### Phase 2: Animation + Expressions (P1/P2)

12. **Keyframe animation** (`animation.rs`): `KeyframeTrack`, `TrackId`, `Keyframe`, `InterpolationMode` (Step, Linear), `Extrapolation` (Hold, DefaultValue). Sampling with binary search. Track validation.
13. **Expression engine** (`expr.rs`, `expr/`): Adapt existing parser. New `ExprNode` AST, `ExpressionValue`, built-in globals (`frame`, `time`, `fps`, `width`, `height`), all built-in math functions. Node-property-based scope. Evaluation integrated into render pipeline.
14. **Property resolution**: Implement precedence chain (expression > keyframe > static) in render evaluation.

### Phase 3: Media Integration (P2)

15. **Media traits** (`media.rs`): Updated `MediaStore`, `ImageResolver`, `VideoFrameResolver` traits with `Result` return types.
16. **MediaIn node** (`node/media_in.rs`): Image source (resolve → decode → `RasterFrame`), Video source (frame mapping with range/speed/loop, resolve via `VideoFrameResolver`).
17. **Asset cache** (`cache.rs`): `AssetCache` for decoded images and video metadata. Thread-safe via `RwLock`.
18. **Runtime capability profile** (`capability.rs`): `RuntimeCapabilityProfile`, composition validation against profile.
19. **FFmpeg adapter** (`ffmpeg.rs`): Update existing FFmpeg code to implement new `VideoFrameResolver` trait. Retain LRU cache and decode worker.

### Phase 4: JSON Delegate (P2)

20. **JSON schema** (`json/schema.rs`): `lumen_graph_v1` serde types mirroring `Composition` → `Graph` → `Node[]` → `Connection[]` → `KeyframeTrack[]`.
21. **JSON conversion** (`json/convert.rs`): Schema → `Composition` conversion with structured validation and expression parsing.
22. **JSON delegate** (`json.rs`): `Composition::from_json()` / `impl TryFrom<&str> for Composition` public API (behind `json` feature), schema revision check, error/warning aggregation.

### Phase 5: Multithreading (P3)

23. **Threading** (`threading.rs`): `RenderOrchestrator` distributing frame jobs to workers via `crossbeam_channel::bounded`. Workers reuse `render_frame`. Ordered sink submission via reorder buffer. Cancellation propagation. Feature-gated behind `threading`.

### Phase 6: Memo + Advanced Caching (P3)

24. **Memo node** (`node/memo.rs`): Memoization eligibility analysis (frame-static, expression policy, subgraph signature). `MemoCache` for persistent cross-session bitmap storage. Cache read/write logic.
25. **Node output cache** (`cache.rs`): Per-session fan-out cache keyed by `(NodeId, frame, resolution, graph_revision)`.

### Phase 7: Dependent Crate Updates

26. **lumen-wasm**: Update to new `Composition` + `render_frame` API. Canvas sink.
27. **lumen-local**: Update to new API. Single-threaded or multithreaded render path.
28. **lumen-server**: Update to new API. Multithreaded render with FFmpeg sink.
29. **Cargo.toml**: Update workspace dependencies, feature flags.

## Complexity Tracking

No constitution violations. All decisions follow minimal-complexity principles:

- Enum dispatch over trait objects (simpler, faster, exhaustive matching)
- Frame-level parallelism over node-level (simpler synchronization)
- Full replacement over deprecation shims (constitution principle V: no dead code or dual-path compatibility)
- Single CPU render path over multi-backend dispatch (v1 scope)
