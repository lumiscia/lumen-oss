# Research: Lumen/Next Compositing Engine

**Feature**: 002-lumen-next-engine
**Date**: 2026-02-23

## R1: Migration Strategy — Full Replacement, No Deprecation

**Decision**: Complete replacement of the existing clip/layer compositor with the node-graph engine. All legacy types (`ClipType`, `Layer`, `Scene`, `GroupClip`, `LayoutClip`, `ShapeClip`, `TextClip`, `ImageClip`, `VideoClip`, `StyleProperty<T>`, `Sequence<T>`, `StyleExpression<T>`) are removed. No compatibility shims, no dual-path rendering.

**Rationale**: The architecture shift from flat `Vec<Layer>` → node graph is fundamentally incompatible. Maintaining both paths would double test surface, introduce ambiguous import paths, and block clean API design. The JSON delegate will be rewritten to target the new graph model, so the `chat_story_v1` schema can be migrated or replaced entirely.

**Alternatives considered**:
- Gradual deprecation with adapter layer — rejected because the data models share no structural overlap (layers vs. nodes), so adapters would be throwaway code.
- Feature-flag gated legacy path — rejected because the user explicitly requires full migration with no fallbacks.

**Migration impact on dependent crates**:
- `lumen-wasm`: Must update to new `Composition` + node graph API. Canvas sink replaces direct `render_scene` call.
- `lumen-local`: Must update to new render session API with `SurfacePool` + `MediaOutput` sink.
- `lumen-server`: Must update to new multithreaded render pipeline with ordered sink submission.

---

## R2: Rust Module Organization — Idiomatic Flat-Module Crate Layout

**Decision**: Use a flat-module layout with `mod.rs`-free style (Rust 2018+ file-as-module). Each major subsystem gets its own top-level module file or directory. Prefer file-per-type only when a module exceeds ~500 lines.

**Rationale**: Flat modules reduce nesting, improve discoverability, and align with Rust ecosystem conventions (serde, tokio, wgpu all use this pattern). The `mod.rs` convention is deprecated in favor of `module_name.rs` + `module_name/` when submodules are needed.

**Module layout**:
```
crates/lumen/src/
├── lib.rs                  # Public API surface, re-exports
├── composition.rs          # Composition, TimelineSettings, RenderSettings
├── node.rs                 # Node, NodeId, NodeKind, port types, property types
├── node/                   # Per-node-kind implementations (when node.rs gets large)
│   ├── transform.rs
│   ├── merge.rs
│   ├── media_in.rs
│   ├── solid_color.rs
│   ├── text.rs
│   ├── shape.rs
│   ├── shape_renderer.rs
│   ├── crop.rs
│   ├── resize.rs
│   ├── blur.rs
│   ├── shadow.rs
│   ├── boolean.rs
│   ├── switch.rs
│   ├── frame_hold.rs
│   ├── media_output.rs
│   └── memo.rs
├── graph.rs                # Graph structure, connections, topological sort, validation
├── render.rs               # RenderContext, frame evaluation, graph traversal
├── raster.rs               # RasterFrame enum, Bitmap, Surface wrappers
├── surface_pool.rs         # SurfacePool, SurfaceRef (RAII)
├── animation.rs            # KeyframeTrack, Keyframe, interpolation, extrapolation, sampling
├── expr.rs                 # Expression AST, evaluator, built-in globals/functions
├── expr/                   # (if expression engine grows)
│   ├── ast.rs
│   ├── eval.rs
│   └── builtins.rs
├── media.rs                # MediaStore, VideoFrameResolver, ImageResolver traits
├── cache.rs                # AssetCache, NodeOutputCache, MemoCache
├── capability.rs           # RuntimeCapabilityProfile, validation against profile
├── error.rs                # All error types: GraphValidationError, RenderError, etc.
├── sink.rs                 # Sink trait, BitmapSink, platform-specific sinks
├── threading.rs            # Orchestrator, worker pool, ordered frame submission
├── json.rs                 # JSON delegate (feature-gated)
├── json/                   # (if JSON module grows)
│   └── ...
└── ffmpeg.rs               # FFmpeg adapter (feature-gated)
```

**Alternatives considered**:
- Deep nesting (`engine/graph/node/kinds/transform.rs`) — rejected, too many levels for a single-crate library.
- Single-file-per-module (`render/mod.rs` + `render/context.rs`) — rejected in favor of Rust 2018+ `render.rs` + `render/` when subdivision is needed.

---

## R3: Rust Patterns and Best Practices

**Decision**: Apply the following Rust patterns consistently across the crate.

### API style — associated functions over free functions
- **Prefer methods and associated functions on types** over free-standing functions. E.g., `Composition::from_json(str)` not `convert_json(str)`; `composition.render_frame(frame, ctx)` not `render_frame(&composition, frame, ctx)`; `graph.validate()` not `validate_graph(&graph)`.
- **Use `From`/`TryFrom` impls** for type conversions where Rust conventions apply. E.g., `impl TryFrom<&str> for Composition` (behind `json` feature).
- **Rationale**: Co-locates behavior with the type it operates on. Familiar to developers coming from OOP backgrounds. Improves discoverability via IDE autocompletion on the type.

### Node architecture — trait + enum dispatch (Option D)
- Each node kind is its own **struct in its own file** (e.g., `node/blur.rs` → `pub struct Blur { pub radius: f32 }`).
- A **`NodeEval` trait** defines the contract: `input_port_defs()`, `output_port_defs()`, `evaluate(&self, inputs: &NodeInputs, ctx: &mut RenderContext) -> Result<PortValue, LumenError>`.
- Every node struct **implements `NodeEval`**, so the compiler enforces the contract per-struct.
- **`NodeKind`** is a `#[non_exhaustive]` enum where each variant wraps its struct: `NodeKind::Blur(blur::Blur)`, `NodeKind::Merge(merge::Merge)`, etc.
- `NodeKind` delegates to each struct's `NodeEval` impl via **exhaustive `match`** — no `dyn` dispatch, no vtable. Adding a new variant without wiring up the match arm is a compile error.
- **Two compile-time safety nets**: the trait enforces the contract per-struct; the enum match enforces exhaustive dispatch.
- **`NodeInputs`** wraps a `HashMap<&'static str, PortValue>` with typed accessors: `get_raster("input")`, `get_raster_optional("mask")`, `get_vector("shape")`.
- **Alternatives considered**:
  - Typed trait with associated types (Option A) — rejected, not object-safe, can't store heterogeneous nodes in `HashMap`.
  - Untyped trait with dyn dispatch (Option B) — rejected, vtable overhead in hot render loop, no exhaustiveness checking.
  - No trait, just match (Option C) — rejected, no compile-time enforcement that every struct implements the required methods.

### Type system
- **Newtype wrappers** for IDs: `NodeId(u64)`, `TrackId(u64)`, `EdgeId(u64)`. Derive `Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord`. Implement `Display`. Use `serde(transparent)` when JSON feature is enabled.
- **Typed port descriptors**: `PortKind { RasterFrame, Surface, Vector }`. `InputPortDef` / `OutputPortDef` structs carry name + kind + optional flag. Connections carry port kinds for compile-time-adjacent validation.
- **`#[non_exhaustive]`** on public enums that may gain variants (BlendMode, NodeKind, ErrorKind).

### Ownership and borrowing
- **`RasterFrame` as owned value** passed through graph evaluation. Nodes consume inputs and produce outputs (move semantics). Fan-out nodes clone `Arc<Bitmap>` for the bitmap variant or produce multiple surfaces from pool.
- **`SurfaceRef` as RAII guard** — on drop, clears and returns surface to pool. Prevents surface leaks.
- **`RenderContext` passed as `&mut`** to evaluation — carries mutable access to caches, pool, and cancellation state.
- **`Arc` for shared immutable data** (decoded images, bitmap cache entries, composition graph during multithreaded render).

### Error handling
- **`thiserror` for all error types**. Each error category is its own enum. Top-level `LumenError` wraps them.
- **No `unwrap()` or `expect()` in library code** — all fallible operations return `Result`.
- **Structured context on errors** via fields, not string formatting. E.g., `GraphValidationError::Cycle { path: Vec<NodeId> }`.

### Performance patterns
- **`SmallVec`** for node input/output lists (most nodes have 1-3 inputs).
- **`parking_lot::Mutex`/`RwLock`** for synchronization (faster than std for uncontended cases).
- **`crossbeam-channel`** for bounded MPSC in threading pipeline.
- **Inline `#[cold]` on error paths** to keep hot paths compact.
- **`repr(u8)`** on small enums (BlendMode, PortType, InterpolationMode) to minimize memory.

### Testing patterns
- **Unit tests in same file** (`#[cfg(test)] mod tests`) for per-module logic.
- **Integration tests in `tests/`** for graph construction → render → pixel verification.
- **Snapshot tests** for deterministic render output (hash-based or reference image comparison).
- **Property-based tests** (`proptest`) for expression evaluation and keyframe interpolation.
- **Mock implementations** of `MediaStore`, `VideoFrameResolver`, `ImageResolver` for testing without real media files.

### API surface
- **`pub` only on the crate's intended API** in `lib.rs`. Internal modules use `pub(crate)`.
- **Builder pattern** for `Composition` construction (many optional fields).
- **`impl From<X> for Y`** for lossless conversions (e.g., `BlendMode` → `skia_safe::BlendMode`).
- **No public `unsafe`** in v1.

---

## R4: Expression Engine — Retain and Extend

**Decision**: The existing hand-written Pratt parser + AST evaluator in `expr/mod.rs` is well-structured and can be adapted. Retain the core parsing/evaluation architecture but:
1. Remove `StyleExpression<T>` wrapper and string-based deferred parsing. Expressions are always pre-parsed at construction/deserialization time.
2. Add missing builtins: `pow`, `mod`, `fract`, `smoothstep`, `text_height`, `text_width`, `uppercase`, `lowercase`.
3. Replace `ExpressionScope` (clip-property-based) with node-property-based scope matching the new graph model.
4. Add `ExpressionValue::String` support for text functions.

**Rationale**: Writing a new expression engine from scratch would duplicate significant tested logic. The existing lexer, parser, and AST walker are solid. The changes needed are in the scope/context layer and built-in function set.

**Alternatives considered**:
- Embed a scripting language (Rhai, mun) — rejected, too heavy for property expressions. The built-in function set is small and fixed.
- Code-generate expression evaluators — rejected, unnecessary complexity for v1 performance targets.

---

## R5: JSON Delegate — New Schema for Node Graph

**Decision**: Replace the `chat_story_v1` schema with a new schema (`lumen_graph_v1`) that directly serializes the node graph model. The new schema maps 1:1 to `Composition` → `Graph` → `Node[]` → `Connection[]` → `KeyframeTrack[]`.

**Rationale**: The `chat_story_v1` schema is tightly coupled to the layer/clip model. Converting between clip-model JSON and graph-model runtime would require a lossy, complex mapping layer. A clean schema that mirrors the runtime model is simpler, more maintainable, and enables round-trip serialization.

**Schema evolution**: The JSON delegate retains a `schema_revision` field for future versioning. The `lumen_graph_v1` schema is the first revision.

---

## R6: Skia Surface Management and RasterFrame

**Decision**: `RasterFrame` is an enum with two variants:
- `Bitmap(Arc<Vec<u8>>, u32, u32)` — immutable pixel data (width, height), reference-counted for fan-out.
- `Surface(SurfaceRef)` — mutable Skia surface from pool, consumed by compositing nodes.

`SurfaceRef` wraps a `skia_safe::Surface` and a reference back to `SurfacePool`. On drop, the surface is cleared and returned to the pool.

**Rationale**: This matches ARCHITECTURE.md exactly. The dual representation avoids unnecessary pixel readback (surface → bitmap) when downstream nodes can composite directly onto surfaces.

**Promotion rule**: Nodes that need mutable drawing (Merge, Transform, ShapeRenderer) promote `Bitmap` inputs to `Surface` via `SurfacePool::acquire` + pixel copy.

---

## R7: Multithreading Model — Frame-Parallel with Crossbeam

**Decision**: Use `crossbeam-channel` for bounded MPSC between orchestrator → workers and workers → sink. Use `parking_lot` for shared state (media resolvers, caches). Workers reuse the single-threaded graph evaluator — parallelism is at the frame-job level, not the node level.

**Rationale**: Frame-level parallelism is simple, correct, and sufficient for v1. Node-level parallelism introduces complex scheduling and synchronization for marginal gains on CPU-bound workloads. Frame-level parallelism scales linearly with core count for multi-frame renders.

**Threading primitives**:
- `crossbeam_channel::bounded` for job queue (orchestrator → workers) and result queue (workers → sink).
- `Arc<parking_lot::Mutex<T>>` for shared video resolver state.
- `Arc<parking_lot::RwLock<T>>` for shared image cache (read-heavy).
- `CancellationToken` (custom or from `tokio_util` if already in deps, otherwise a simple `AtomicBool`).

---

## R8: Cache Layer Design

**Decision**: Three distinct cache layers, each with clear ownership:

1. **AssetCache** (shared, long-lived): Stores decoded images, video metadata, resolver handles. Keyed by source path/ID. Survives across render sessions. Thread-safe via `RwLock`.

2. **NodeOutputCache** (per render session): Caches evaluated `RasterFrame` outputs for fan-out reuse within a single frame render. Keyed by `(NodeId, frame, resolution, graph_revision)`. Cleared between sessions.

3. **MemoCache** (persistent, cross-session): Stores bitmap-backed results for `Memo` nodes. Keyed by `(CacheId, dimensions, subgraph_signature_hash)`. Persisted in memory (v1) or optionally to disk (post-v1).

**Rationale**: Separating cache layers by lifetime and purpose avoids key collisions and simplifies invalidation. Asset cache is truly shared; node output cache is ephemeral; memo cache is user-controlled.

---

## R9: Legacy Code Removal Inventory

**Decision**: The following files/modules are deleted entirely with no replacement:

| File/Module | Reason |
|---|---|
| `src/scene.rs` | Replaced by `composition.rs` (node graph, not layer list) |
| `src/clip/` (entire directory) | Replaced by `node/` (node-based, not clip-based) |
| `src/clip/style/` (entire directory) | Replaced by node properties + `animation.rs` |
| `src/dependency/` (entire directory) | Replaced by `graph.rs` topological sort |
| `src/render/backend/software.rs` | Replaced by unified CPU render path in `render.rs` |
| `src/render/backend/mod.rs` | GPU stubs removed; `RenderBackend` trait removed (single CPU path) |
| `src/json/enabled.rs` | `chat_story_v1` schema replaced by `lumen_graph_v1` |

**Files retained and adapted**:

| File/Module | Changes |
|---|---|
| `src/lib.rs` | Rewritten with new module structure |
| `src/media.rs` | Traits retained, signatures updated for new types |
| `src/expr/mod.rs` | Parser/evaluator retained, scope/builtins updated |
| `src/time.rs` | `Rational` may be retained or replaced by simple `fps: f32` |
| `src/render/context.rs` | Rewritten as `RenderContext` with new fields |
| `src/ffmpeg/` | Adapter updated for new `VideoFrameResolver` trait |
| `src/json/mod.rs` | Delegate framework retained, schema replaced |

**Dependent crate updates** (not in this crate but must be coordinated):
- `lumen-wasm/src/lib.rs` — update to new API
- `lumen-local/src/main.rs` — update to new API
- `lumen-server/src/` — update to new API

---

## R10: WASM Emscripten Dependency Compatibility

**Decision**: All three planned dependencies (`parking_lot 0.12`, `crossbeam-channel 0.5`, `smallvec 1.15`) compile correctly on `wasm32-unknown-emscripten` and require no special handling. Keep `crossbeam-channel` as the threading channel primitive; do not replace with `crossfire-rs`.

**Rationale**:

Verified by creating a test crate targeting `wasm32-unknown-emscripten` with all three dependencies:

- **smallvec 1.15**: Pure Rust, no platform-specific code. Compiles cleanly.
- **parking_lot 0.12**: Compiles cleanly. Key finding: `wasm32-unknown-emscripten` sets both `target_family = "unix"` and `target_family = "wasm"` plus the `unix` cfg. In `parking_lot_core`'s `cfg_if` chain (`thread_parker/mod.rs`), the `#[cfg(unix)]` branch fires before `#[cfg(target_family = "wasm")]`. This means parking_lot uses the **unix/pthreads thread parker** on emscripten (real mutexes via emscripten's libc), not the wasm stub (`thread_parker/wasm.rs` which panics on any park attempt). `Mutex` and `RwLock` function as real synchronization primitives on emscripten.
- **crossbeam-channel 0.5**: No wasm-specific code paths. Relies on std thread primitives which work on emscripten via pthreads. Compiles cleanly.

This validates the spec's approach: `parking_lot` is used unconditionally (for `SurfacePool` Mutex, `AssetCache` RwLock), while `crossbeam-channel` is behind the `threading` feature gate. Both work on emscripten even though `lumen-wasm` only uses single-threaded rendering in v1.

**crossfire-rs evaluation**: `crossfire v3.1.3` was evaluated as a potential `crossbeam-channel` replacement. It compiles on emscripten and shares dependencies (`parking_lot`, `crossbeam-utils`, `smallvec`). However, it is rejected because:
1. `crossbeam-channel` already works on emscripten — no compatibility problem to solve.
2. crossfire's API is significantly more complex: separate types for single-producer vs multi-producer (`Tx`/`MTx`), blocking vs async (`Tx`/`AsyncTx`), and channel flavor generics (`Array`, `List`, `One`).
3. crossfire has no wasm testing in CI — only x86_64 and ARM are tested.
4. We don't need async channel support (the threading pipeline is fully synchronous).
5. `crossbeam-channel` is more mature and widely used as part of the crossbeam ecosystem.

**Alternatives considered**:
- `crossfire-rs` as crossbeam-channel replacement — rejected (see above).
- `flume` as crossbeam-channel replacement — not evaluated; crossbeam-channel works, no reason to switch.
- Conditional compilation to swap channel impl on wasm — unnecessary since crossbeam-channel compiles and the `threading` feature is not enabled for wasm targets anyway.
