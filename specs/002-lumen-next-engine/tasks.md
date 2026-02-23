# Tasks: Lumen/Next Compositing Engine

**Input**: Design documents from `/specs/002-lumen-next-engine/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Included per user story — each behavior-changing story has dedicated test tasks.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Crate root**: `crates/lumen/`
- **Source**: `crates/lumen/src/`
- **Tests**: `crates/lumen/tests/`
- **Node implementations**: `crates/lumen/src/node/`
- **Expression engine**: `crates/lumen/src/expr/`
- **JSON delegate**: `crates/lumen/src/json/`

---

## Phase 1: Setup

**Purpose**: Delete all legacy code, update Cargo.toml, establish new module skeleton

- [X] T001 Delete all legacy source files: `crates/lumen/src/scene.rs`, `crates/lumen/src/time.rs`, entire `crates/lumen/src/clip/` directory, entire `crates/lumen/src/dependency/` directory, `crates/lumen/src/render/backend/` directory (including `software.rs` and `mod.rs`), `crates/lumen/src/render/context.rs`, `crates/lumen/src/render/mod.rs`, `crates/lumen/src/json/enabled.rs`
- [X] T002 Update `crates/lumen/Cargo.toml`: remove `taffy` dependency, remove `gpu-metal` and `gpu-vulkan` features and their deps (`ash`, `objc2`, `objc2-metal`), add `parking_lot`, `crossbeam-channel`, `smallvec` to dependencies, add `threading` feature gate for `crossbeam-channel` + `parking_lot`, keep `skia-safe`, `thiserror`, `ffmpeg-next` (feature: ffmpeg), `serde`/`serde_json`/`anyhow` (feature: json)
- [X] T003 Create empty module skeleton files with minimal `pub(crate)` module declarations: `crates/lumen/src/lib.rs` (rewritten with new module tree), `crates/lumen/src/error.rs`, `crates/lumen/src/composition.rs`, `crates/lumen/src/graph.rs`, `crates/lumen/src/node.rs`, `crates/lumen/src/node/` directory, `crates/lumen/src/raster.rs`, `crates/lumen/src/surface_pool.rs`, `crates/lumen/src/render.rs`, `crates/lumen/src/animation.rs`, `crates/lumen/src/expr.rs`, `crates/lumen/src/expr/` directory, `crates/lumen/src/media.rs`, `crates/lumen/src/cache.rs`, `crates/lumen/src/capability.rs`, `crates/lumen/src/sink.rs`, `crates/lumen/src/threading.rs`, `crates/lumen/src/json.rs`, `crates/lumen/src/json/` directory, `crates/lumen/src/ffmpeg.rs`. Each file should have a module-level doc comment and compile as an empty module. Verify `cargo check -p lumen` passes.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and infrastructure that ALL user stories depend on. No rendering or node logic — just the type system, graph structure, raster primitives, and error handling.

**CRITICAL**: No user story work can begin until this phase is complete.

- [ ] T004 [P] Implement all error types in `crates/lumen/src/error.rs`: `LumenError` top-level enum wrapping `GraphValidationError`, `PropertyError`, `ExpressionError`, `MediaError`, `RenderError`, `ThreadingError`, `SinkError`. Each variant must carry structured context fields (node_id, node kind, property_path, frame, source). Use `thiserror` derive. Add `Warning` enum for non-fatal diagnostics. No `unwrap()`/`expect()`.
- [ ] T005 [P] Implement `NodeId`, `TrackId` newtypes, `PortKind` enum, `PortValue` enum, `InputPortDef`/`OutputPortDef` structs, `NodeInputs` struct with typed accessors (`get_raster`, `get_raster_optional`, `get_vector`), `NodeEval` trait (`input_port_defs`, `output_port_defs`, `evaluate`), `PropertyValue` enum, `BlendMode` enum (with `impl From<BlendMode> for skia_safe::BlendMode`), and `NodeKind` enum (all 16 variants as stubs wrapping unit structs) in `crates/lumen/src/node.rs`. Derive standard traits on IDs. Use `#[non_exhaustive]` on `NodeKind` and `BlendMode`. Use `#[repr(u8)]` on small enums.
- [ ] T006 [P] Implement `Composition`, `TimelineSettings`, `RenderSettings` in `crates/lumen/src/composition.rs`. `Composition` holds `Graph`, `TimelineSettings`, `RenderSettings`, `Vec<KeyframeTrack>`, `Option<CompositionMetadata>`. Add `Composition::new()`, `add_track()`, `validate()` (delegates to `graph.validate()` + track validation). Add `time_seconds(frame) -> f64` helper on `TimelineSettings`.
- [ ] T007 [P] Implement `Graph`, `Connection`, `InputPort`, `OutputPort` in `crates/lumen/src/graph.rs`. `Graph` stores `HashMap<NodeId, Node>` and `Vec<Connection>`. Implement `Graph::new()`, `add_node()`, `connect()`, `remove_node()`, `remove_connection()`. Implement `Graph::validate()`: cycle detection (Kahn's algorithm), port type compatibility, required input connectivity, Switch range overlap check, exactly-one-MediaOutput check. Implement `Graph::evaluation_order(target: NodeId) -> Result<Vec<NodeId>, LumenError>` (topological sort from target, dead node elimination).
- [ ] T008 [P] Implement `RasterFrame` enum (`Bitmap(Arc<Vec<u8>>, u32, u32)` | `Surface(SurfaceRef)`) in `crates/lumen/src/raster.rs`. Add `dimensions()`, `as_bitmap_bytes()`, `to_bitmap()`, `promote_to_surface(pool: &SurfacePool)` methods. `Bitmap` variant uses `Arc` for cheap fan-out cloning.
- [ ] T009 [P] Implement `SurfacePool` and `SurfaceRef` in `crates/lumen/src/surface_pool.rs`. `SurfacePool` wraps `Mutex<HashMap<(u32, u32), Vec<skia_safe::Surface>>>`. `acquire(width, height) -> SurfaceRef` returns pooled or new surface. `SurfaceRef` implements `Drop` to clear and return surface to pool. Pool grows on demand, never fails.
- [ ] T010 [P] Implement `Sink` trait in `crates/lumen/src/sink.rs`: `write_frame(&mut self, frame: u32, data: &RasterFrame) -> Result<(), SinkError>`, `finalize(&mut self) -> Result<(), SinkError>`. Add `BitmapSink` test helper that collects frames into a `Vec`.
- [ ] T011 [P] Implement `MediaStore`, `ImageResolver`, `VideoFrameResolver` traits in `crates/lumen/src/media.rs`. Each trait requires `Send + Sync`. `ImageResolver`: `id()`, `width()`, `height()`, `resolve() -> Result<Vec<u8>, MediaError>`. `VideoFrameResolver`: `id()`, `width()`, `height()`, `frame_count()`, `resolve_frame(frame: u32) -> Result<Vec<u8>, MediaError>`. Add `MockMediaStore`, `MockImageResolver` test helpers with `#[cfg(test)]`.
- [ ] T012 [P] Implement `RuntimeCapabilityProfile` in `crates/lumen/src/capability.rs`: `has_image_resolver`, `has_video_resolver`, `has_threading`, `sink_types: Vec<SinkType>`. Add `RuntimeCapabilityProfile::cpu_only()` convenience constructor. Add `Composition::validate_against_profile()` method that checks composition node requirements against profile capabilities.
- [ ] T013 Implement `RenderContext` and `Composition::render_frame()` shell in `crates/lumen/src/render.rs`. `RenderContext` holds frame, fps, width, height, duration_frames, `Arc<SurfacePool>`, `Arc<RwLock<AssetCache>>`, `HashMap<NodeId, PortValue>` (node output cache), `Arc<dyn MediaStore>`, `RuntimeCapabilityProfile`, `CancellationToken`. Implement `Composition::render_frame(&self, frame: u32, ctx: &mut RenderContext) -> Result<RasterFrame, LumenError>` that calls `graph.evaluation_order()`, iterates in topo order, populates `NodeInputs` from cached upstream outputs, calls `node.kind.evaluate(inputs, ctx)`, caches result, and extracts final `RasterFrame` from `MediaOutput` node.
- [ ] T014 Rewrite `crates/lumen/src/lib.rs` with new public API re-exports per contracts/public-api.md. Declare all modules with appropriate `pub` / `pub(crate)` visibility. Feature-gate `json`, `ffmpeg`, `threading` modules. Verify `cargo check -p lumen` passes with no errors.

**Checkpoint**: Core type system compiles. Graph can be constructed, validated, and traversed. RasterFrame and SurfacePool work. Render pipeline shell dispatches to node evaluate stubs. No rendering output yet (nodes are stubs).

---

## Phase 3: User Story 1 — Render a Static Composition (Priority: P1) MVP

**Goal**: A developer can build a node graph with source nodes, transforms, effects, and compositing, then render a single frame to a correct RGBA8 premultiplied sRGB bitmap.

**Independent Test**: Construct SolidColor → Transform → MediaOutput, render frame 0, verify output pixels.

### Tests for User Story 1

- [ ] T015 [P] [US1] Write integration test in `crates/lumen/tests/graph_validation.rs`: test cycle detection rejects cyclic graph, test missing required input rejects graph, test port type mismatch rejects graph, test valid SolidColor→Transform→MediaOutput graph passes validation, test overlapping Switch ranges rejected. Each test verifies structured error context (node_id, node kind).
- [ ] T016 [P] [US1] Write integration test in `crates/lumen/tests/render_basic.rs`: test SolidColor→MediaOutput renders correct solid color bitmap, test SolidColor→Transform(translate)→MediaOutput renders translated rectangle, test Merge(SolidColor, SolidColor, opacity=0.5)→MediaOutput produces correct blended output with premultiplied alpha, test Shape→ShapeRenderer→MediaOutput renders filled rectangle.

### Implementation for User Story 1

- [ ] T017 [P] [US1] Implement `SolidColor` node in `crates/lumen/src/node/solid_color.rs`: struct with `color: [u8; 4]`, `width: Option<u32>`, `height: Option<u32>`. `impl NodeEval`: produces `RasterFrame::Bitmap` filled with solid color at composition dimensions (or custom dimensions). Wire into `NodeKind::SolidColor(SolidColor)` variant and `NodeKind::evaluate()` match arm in `node.rs`.
- [ ] T018 [P] [US1] Implement `Shape` node in `crates/lumen/src/node/shape.rs`: struct with `ShapeGeometry` enum (`Rectangle { width, height }`, `Ellipse { width, height }`, `Polygon { points }`) . `impl NodeEval`: outputs `PortValue::Vector(VectorData)`. Define `VectorData` type to carry shape geometry.
- [ ] T019 [P] [US1] Implement `ShapeRenderer` node in `crates/lumen/src/node/shape_renderer.rs`: struct with `fill_color`, `stroke_color`, `stroke_width`, `fill_enabled`, `stroke_enabled`. `impl NodeEval`: takes `Vector` input, rasterizes to `RasterFrame` using Skia `Path` + `Paint` for fill/stroke.
- [ ] T020 [P] [US1] Implement `Text` node in `crates/lumen/src/node/text.rs`: struct with `content`, `font_family`, `font_size`, `font_weight`, `font_style`, `max_width`, `color`, `alignment` (horizontal + vertical). `impl NodeEval`: uses Skia `ParagraphBuilder` + `ParagraphStyle` to layout and render text to a `RasterFrame`. Adapt text layout logic from legacy `clip/text.rs`.
- [ ] T021 [P] [US1] Implement `MediaOutput` node in `crates/lumen/src/node/media_output.rs`: struct (no properties). `impl NodeEval`: passes through input `RasterFrame`, converting Surface-backed to Bitmap-backed at the sink boundary via `to_bitmap()`.
- [ ] T022 [US1] Implement `Transform` node in `crates/lumen/src/node/transform.rs`: struct with `scale_x`, `scale_y`, `translate_x`, `translate_y`, `rotate`, `pivot_x`, `pivot_y`. `impl NodeEval`: takes `RasterFrame` input, promotes to Surface if needed, applies Skia matrix transform (Scale → Rotate → Translate order), bilinear sampling default. Pivot defaults to center of input bounds.
- [ ] T023 [P] [US1] Implement `Crop` node in `crates/lumen/src/node/crop.rs`: struct with `x`, `y`, `width`, `height`. `impl NodeEval`: takes `RasterFrame`, produces cropped `RasterFrame`.
- [ ] T024 [P] [US1] Implement `Resize` node in `crates/lumen/src/node/resize.rs`: struct with `width`, `height`, `mode` (Stretch/Fit/Fill), `sampling` (Nearest/Bilinear). `impl NodeEval`: takes `RasterFrame`, produces resized `RasterFrame`.
- [ ] T025 [P] [US1] Implement `Blur` node in `crates/lumen/src/node/blur.rs`: struct with `radius: f32`. `impl NodeEval`: takes `RasterFrame`, applies Gaussian blur via Skia `MaskFilter::blur()` or image filter, produces `RasterFrame`.
- [ ] T026 [P] [US1] Implement `Shadow` node in `crates/lumen/src/node/shadow.rs`: struct with `color`, `blur_radius`, `offset_x`, `offset_y`. `impl NodeEval`: takes `RasterFrame`, generates shadow from input alpha, offsets shadow, composites shadow beneath input using premultiplied alpha. Produces `RasterFrame`.
- [ ] T027 [US1] Implement `Merge` node in `crates/lumen/src/node/merge.rs`: struct with `blend_mode: BlendMode`, `opacity: f32`. `impl NodeEval`: takes "base" + "overlay" + optional "mask" `RasterFrame` inputs. Promotes base to Surface. Applies opacity to overlay, blends overlay onto base using Skia blend mode. If mask present, modulates overlay contribution. Premultiplied alpha semantics throughout.
- [ ] T028 [US1] Implement `Boolean` node in `crates/lumen/src/node/boolean.rs`: struct with `mask_kind` (`BooleanMaskKind` enum: `ShapeMask { shape: ShapeGeometry }` | `RasterMask { source }`) and `invert: bool`. `impl NodeEval`: takes "source" `RasterFrame`, applies shape or raster mask, inverts if flagged. Alpha-based mask interpretation. Produces masked `RasterFrame`.
- [ ] T029 [P] [US1] Implement `Switch` node in `crates/lumen/src/node/switch.rs`: struct with `map: HashMap<u16, Range<u32>>` (input index → frame range). `impl NodeEval`: dynamic inputs as indexed "input_0", "input_1", etc. At current frame, selects active input by range lookup. No active range → transparent output. Overlapping ranges caught by graph validation.
- [ ] T030 [P] [US1] Implement `FrameHold` node in `crates/lumen/src/node/frame_hold.rs`: struct with `hold_frame: u32`. `impl NodeEval`: overrides the current frame in `RenderContext` when evaluating upstream, returning the upstream output at the held frame.
- [ ] T031 [US1] Implement graph optimization passes in `crates/lumen/src/render.rs` or `crates/lumen/src/graph.rs`: dead node elimination (prune unreachable from MediaOutput), frame-range culling (skip nodes whose frame range excludes current frame), per-frame fan-out memoization (reuse cached PortValue when one output feeds multiple consumers — already handled by node output cache in RenderContext), basic no-op elimination (identity transform, zero-radius blur).

**Checkpoint**: Can construct a graph with any combination of source/transform/effect/compositing nodes, validate it, render a single frame, and get a correct bitmap. This is the MVP.

---

## Phase 4: User Story 2 — Animate Properties with Keyframes (Priority: P1)

**Goal**: A developer can attach keyframe tracks to node properties and render animated sequences where property values are interpolated per-frame.

**Independent Test**: Create Transform node with linear keyframe on translate_x, render frames 0/15/30, verify positions.

**Depends on**: US1 (needs working render pipeline to verify animated output)

### Tests for User Story 2

- [ ] T032 [P] [US2] Write test in `crates/lumen/tests/animation.rs`: test Linear interpolation on Float (frames 0→60, values 0.0→100.0, sample at 30 returns 50.0), test Step interpolation on Boolean (holds previous key value), test Hold extrapolation before first and after last key, test single-key track returns that value for all frames, test empty keys is invalid track error, test duplicate time_frame rejected, test track targeting non-existent node rejected, test track targeting invalid property_path rejected.

### Implementation for User Story 2

- [ ] T033 [US2] Implement keyframe system in `crates/lumen/src/animation.rs`: `KeyframeTrack` (id, node_id, property_path, value_type, keys, before/after extrapolation), `TrackId` newtype, `Keyframe` (time_frame, value, interpolation), `InterpolationMode` enum (Step, Linear), `Extrapolation` enum (Hold, DefaultValue), `AnimatableType` enum (Float, Int, Boolean, Color, Vector2, String), `PropertyPath` newtype (String, stable dot-separated path). Implement `KeyframeTrack::new()`, `set_key()`, `remove_key()`, `sample(frame) -> PropertyValue` with binary search, interpolation by mode and type, extrapolation rules. Implement track validation (sorted keys, unique time_frame, valid interpolation mode for type, target node/property exists).
- [ ] T034 [US2] Integrate keyframe sampling into render pipeline in `crates/lumen/src/render.rs`: before evaluating each node, resolve animated properties by sampling their keyframe tracks at the current frame. Add `Composition::sample_property()` method. Update node evaluation to use resolved property values instead of static struct fields. This requires a property resolution layer that reads the node's static properties, overlays keyframe-sampled values, and passes resolved properties to `NodeEval::evaluate()`. Consider a `ResolvedProperties` bag or extending `RenderContext` with a property resolver.

**Checkpoint**: Can render multi-frame sequences where node properties animate via keyframes. Interpolated values are frame-accurate.

---

## Phase 5: User Story 3 — Evaluate Expressions at Render Time (Priority: P2)

**Goal**: A developer can assign typed expressions to node properties that reference built-in globals and math functions, evaluated dynamically each frame.

**Independent Test**: Assign `sin(time * 3.14159) * 100` to Transform translate_y, render multiple frames, verify sine wave.

**Depends on**: US2 (needs property resolution pipeline to integrate expression precedence)

### Tests for User Story 3

- [ ] T035 [P] [US3] Write test in `crates/lumen/tests/expressions.rs`: test built-in globals (`frame`=30 at frame 30, `time`=1.0 at frame 30/30fps, `fps`, `width`, `height`), test math builtins (`sin`, `cos`, `lerp`, `clamp`, `smoothstep`, `pow`, `mod`, `fract`, `floor`, `ceil`, `round`, `abs`, `min`, `max`), test text functions (`uppercase`, `lowercase`), test undefined variable produces ExpressionError with node_id and property_path, test operator precedence, test nested function calls.

### Implementation for User Story 3

- [ ] T036 [P] [US3] Implement expression AST types in `crates/lumen/src/expr/ast.rs`: `ExpressionId` newtype, `ExprNode` enum (Literal, Binary, Unary, Builtin, Global, NodeProperty, Conditional), `ExpressionValue` enum (Number(f64), Boolean(bool), String(String)), `BinaryOp`, `UnaryOp`, `GlobalVar` (frame, time, fps, width, height), `BuiltinFn` enum (all 18 builtins: min, max, abs, floor, ceil, round, sin, cos, clamp, lerp, pow, mod, fract, smoothstep, text_height, text_width, uppercase, lowercase), `Expression` struct (id, ast, references).
- [ ] T037 [US3] Adapt expression parser in `crates/lumen/src/expr/parser.rs`: port the existing Pratt parser and lexer from legacy `expr/mod.rs`. Replace `ExpressionScope`/`ExpressionProperty` (clip-based) with node-property references (`NodeId`, `PropertyPath`). Update token set for new builtins. Parser produces `ExprNode` AST. Parsing errors produce `ExpressionError` with source location.
- [ ] T038 [P] [US3] Implement built-in function evaluator in `crates/lumen/src/expr/builtins.rs`: each `BuiltinFn` variant maps to an implementation. Math functions operate on `f64`. `text_height`/`text_width` require access to Skia text layout via `RenderContext`. `uppercase`/`lowercase` operate on `ExpressionValue::String`.
- [ ] T039 [US3] Implement expression evaluator in `crates/lumen/src/expr/eval.rs`: `Expression::evaluate(ctx: &RenderContext) -> Result<ExpressionValue, LumenError>`. Recursive AST walker. Globals resolved from `RenderContext` (frame, time, fps, width, height). Node property references resolved via `Composition::sample_property()`. Builtin calls dispatched to `builtins.rs`. Type coercion rules for arithmetic.
- [ ] T040 [US3] Wire expression module root in `crates/lumen/src/expr.rs`: declare submodules (ast, parser, eval, builtins). Re-export public types. Add `Expression::parse(source: &str) -> Result<Expression, ExpressionError>` as the public entry point.
- [ ] T041 [US3] Integrate expression evaluation into property resolution in `crates/lumen/src/render.rs`: extend the property resolution layer from T034 to implement the full precedence chain: expression > keyframe > static literal. If a property has an expression, evaluate it. Else if it has a keyframe track, sample it. Else use the static value. Expression errors produce structured `LumenError` with node_id and property_path.

**Checkpoint**: Expressions, keyframes, and static values all work together with correct precedence. All 18 built-in functions evaluate correctly.

---

## Phase 6: User Story 4 — Decode and Composite Video Media (Priority: P2)

**Goal**: A developer can use MediaIn nodes with image and video sources, resolving media through platform-provided trait implementations.

**Independent Test**: Mock VideoFrameResolver returning known test frames, render MediaIn(Video)→MediaOutput, verify correct source frame mapping.

**Depends on**: US1 (needs render pipeline), optionally US2 (animated speed/range)

### Tests for User Story 4

- [ ] T042 [P] [US4] Write test in `crates/lumen/tests/render_basic.rs` (append): test MediaIn(Image) with MockImageResolver renders correct image, test MediaIn(Video) at speed 1.0 requests correct source frame, test MediaIn(Video) at speed 2.0 requests frame at 2x rate, test composition with video node + no VideoFrameResolver in profile is rejected by `validate_against_profile()`, test FPS mismatch produces warning (not error).

### Implementation for User Story 4

- [ ] T043 [US4] Implement `MediaIn` node in `crates/lumen/src/node/media_in.rs`: struct with `MediaInKind` enum (`Image { source: String }` | `Video { source: String, range: Option<Range<u32>>, speed: f32, loop_mode: LoopMode }`). `LoopMode` enum: `None`, `Repeat`, `PingPong`. `impl NodeEval`: for Image, resolve via `MediaStore::get_image_resolver()`, decode RGBA8, return `RasterFrame::Bitmap`. For Video, compute source frame from composition frame using range/speed/loop mapping (adapt `map_to_source_frame` from legacy `clip/media.rs`), resolve via `VideoFrameResolver::resolve_frame()`. Convert decoded pixels to internal RGBA8 premultiplied sRGB format.
- [ ] T044 [P] [US4] Implement `AssetCache` in `crates/lumen/src/cache.rs`: `AssetCache` stores decoded images (`HashMap<String, Arc<Vec<u8>>>`) and video metadata. Thread-safe via `RwLock`. `get_or_insert_image(source, resolver)` pattern. Add `NodeOutputCache` stub (per-session, keyed by `NodeId`). Integrate `AssetCache` into `RenderContext`.
- [ ] T045 [US4] Implement FFmpeg adapter update in `crates/lumen/src/ffmpeg.rs` (feature: ffmpeg): adapt existing `LibavStreamDecoder` and `VideoDecodeWorker` from legacy `ffmpeg/mod.rs` and `ffmpeg/worker.rs` to implement the new `VideoFrameResolver` trait. Retain LRU frame cache, HW decode support, prefetch logic, and seek/reopen fallback. Wrap in `FfmpegVideoResolver` struct. Add `FfmpegMediaStore` that creates `FfmpegVideoResolver` instances.

**Checkpoint**: Images and videos composite correctly through the node graph. Platform adapters plug in via traits.

---

## Phase 7: User Story 3b — JSON Delegate (Priority: P2, extends US3/US4)

**Goal**: Compositions can be constructed from JSON input via `Composition::from_json()`, enabling serialization round-trips and editor integration.

**Independent Test**: Parse a `lumen_graph_v1` JSON string into a `Composition`, validate it, render a frame.

**Depends on**: US1 (graph types), US2 (keyframe types), US3 (expression parsing)

### Implementation for JSON Delegate

- [ ] T046 [P] Implement `lumen_graph_v1` serde types in `crates/lumen/src/json/schema.rs` (feature: json): define serde `Deserialize` structs mirroring the Composition model: `JsonComposition`, `JsonGraph`, `JsonNode`, `JsonConnection`, `JsonNodeKind` (each variant maps to a node struct's serialized form), `JsonKeyframeTrack`, `JsonKeyframe`, `JsonExpression` (serialized string form). Add `schema_revision: String` field. Include `#[serde(deny_unknown_fields)]` where appropriate.
- [ ] T047 Implement schema→model conversion in `crates/lumen/src/json/convert.rs` (feature: json): `impl TryFrom<JsonComposition> for Composition`. Convert each JSON node to its `NodeKind` variant. Parse expression strings into `Expression` AST via `Expression::parse()`. Convert keyframe data to `KeyframeTrack`. Validate stable IDs. Produce structured `LumenError` for malformed/invalid input. Collect warnings for recoverable issues.
- [ ] T048 Wire JSON delegate in `crates/lumen/src/json.rs` (feature: json): declare submodules (schema, convert). Implement `Composition::from_json(input: &str) -> JsonDelegateResult` and `impl TryFrom<&str> for Composition`. `JsonDelegateResult` carries status, optional composition, errors, warnings. Schema revision check (`lumen_graph_v1`). Re-export public types.

**Checkpoint**: JSON round-trip works. `Composition::from_json()` parses, validates, and produces a renderable composition.

---

## Phase 8: User Story 5 — Multithreaded Frame-Parallel Rendering (Priority: P3)

**Goal**: Multi-frame renders distribute work across threads with correct ordered output.

**Independent Test**: Render 60 frames with 4 workers, verify all frames arrive in order with pixel-identical output to single-threaded.

**Depends on**: US1 (single-frame render), US4 (media traits need Send+Sync for cross-thread sharing)

### Tests for User Story 5

- [ ] T049 [P] [US5] Write test in `crates/lumen/tests/threading.rs` (feature: threading): test 60-frame render with 4 workers produces frames in order, test output is pixel-identical to single-threaded render for same composition, test cancellation stops workers and finalizes sink, test render error on one frame propagates with frame/node context.

### Implementation for User Story 5

- [ ] T050 [US5] Implement `RenderOrchestrator` in `crates/lumen/src/threading.rs` (feature: threading): distributes frame jobs to workers via `crossbeam_channel::bounded`. Each worker clones `Arc<Composition>` and creates its own `RenderContext` with shared `Arc<SurfacePool>`, `Arc<RwLock<AssetCache>>`, `Arc<dyn MediaStore>`. Workers call `composition.render_frame(frame, &mut ctx)`. Results sent to result channel. Reorder buffer in sink thread ensures ascending frame order before calling `sink.write_frame()`. Implement `Composition::render_sequence()` as the public API. `CancellationToken` (simple `Arc<AtomicBool>`) propagated to all workers and checked between frame jobs. Error propagation: first error cancels remaining jobs, reported with full context.

**Checkpoint**: Multi-frame renders work across threads with correct ordering. Cancellation and error propagation verified.

---

## Phase 9: User Story 6 — Memo Nodes for Cross-Session Caching (Priority: P3)

**Goal**: Memo nodes cache and reuse rendered output of static subgraphs across render sessions.

**Independent Test**: Render with Memo node, modify unrelated node, re-render, verify memoized subgraph was not re-evaluated.

**Depends on**: US1 (render pipeline), US3 (expression eligibility analysis)

### Implementation for User Story 6

- [ ] T051 [US6] Implement `Memo` node in `crates/lumen/src/node/memo.rs`: struct with `cache_id: String`, `allow_expressions: bool`. `impl NodeEval`: 1) Analyze upstream subgraph for memoization eligibility (frame-static check, expression policy, determinism). 2) Compute subgraph signature hash (node types, properties, static expression results, topology). 3) Check `MemoCache` for hit using (cache_id, dimensions, format, signature). 4) On hit: return cached `RasterFrame::Bitmap`. 5) On miss: evaluate source, persist bitmap result to `MemoCache`, return result. 6) If ineligible: pass-through only, no cache write. Validation: `cache_id` must be non-empty.
- [ ] T052 [US6] Implement `MemoCache` in `crates/lumen/src/cache.rs` (extend): persistent cross-session cache. Keyed by `(String, u32, u32, u64)` = (cache_id, width, height, signature_hash). Stores `Arc<Vec<u8>>` (bitmap data). Thread-safe via `RwLock`. Implement `NodeOutputCache` for per-session fan-out cache keyed by `(NodeId, u32, u32, u64)` = (node_id, frame, width*height resolution, graph_revision). Integrate both caches into `RenderContext`.

**Checkpoint**: Memo nodes cache eligible static subgraphs. Cache hits skip re-evaluation. Ineligible subgraphs pass through correctly.

---

## Phase 10: Dependent Crate Updates

**Purpose**: Update all consumers of the `lumen` crate to the new API

- [ ] T053 [P] Update `crates/lumen-wasm` to new `Composition` + `Composition::render_frame()` API. Replace `Scene`/`render_scene` calls. Update JSON delegate to `Composition::from_json()`. Implement or adapt canvas sink for wasm output. Verify `cargo check -p lumen-wasm` passes.
- [ ] T054 [P] Update `crates/lumen-local` to new API. Replace `Scene`/`render_scene` calls with `Composition::render_frame()`. Update FFmpeg integration to use new `FfmpegMediaStore`. Support optional multithreaded render path via `Composition::render_sequence()` (feature: threading). Verify `cargo check -p lumen-local` passes.
- [ ] T055 [P] Update `crates/lumen-server` to new API. Replace `Scene`/`render_scene` calls with `Composition::render_frame()` or `render_sequence()`. Update FFmpeg sink integration. Verify `cargo check -p lumen-server` passes.

**Checkpoint**: All workspace crates compile. `cargo check --workspace` passes.

---

## Phase 11: Polish & Cross-Cutting Concerns

**Purpose**: Final verification across all stories

- [ ] T056 [P] Run `cargo test -p lumen --all-features` and fix any failures
- [ ] T057 [P] Run `cargo clippy -p lumen --all-features` and resolve all warnings
- [ ] T058 [P] Verify `cargo check --workspace` passes (all dependent crates)
- [ ] T059 Validate quickstart.md code example compiles and runs correctly against the implemented API
- [ ] T060 Final review: confirm no legacy code remains (no references to `Scene`, `Layer`, `ClipType`, `StyleProperty`, `chat_story_v1`, `RenderBackend`, `StreamingAssets` anywhere in `crates/lumen/src/`)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — MVP milestone
- **US2 (Phase 4)**: Depends on US1 (needs render pipeline)
- **US3 (Phase 5)**: Depends on US2 (needs property resolution)
- **US4 (Phase 6)**: Depends on US1, can start in parallel with US2/US3
- **JSON (Phase 7)**: Depends on US1+US2+US3 (needs all types)
- **US5 (Phase 8)**: Depends on US1+US4 (needs render pipeline + Send+Sync traits)
- **US6 (Phase 9)**: Depends on US1+US3 (needs render pipeline + expression analysis)
- **Dependent Crates (Phase 10)**: Depends on all user stories
- **Polish (Phase 11)**: Depends on all phases

### User Story Dependencies

```
Setup → Foundational → US1 (MVP) → US2 → US3 → JSON
                          ↓           ↓        ↓
                          US4 --------+→ US5   US6
                                       ↓
                               Dependent Crates → Polish
```

### Within Each User Story

- Tests written first (fail before implementation)
- Core types before behavior
- Node implementations before integration
- Story complete before checkpoint

### Parallel Opportunities

- **Phase 2**: T004–T012 are all [P] — different files, no deps on each other
- **Phase 3 (US1)**: T017–T021, T023–T026, T029–T030 are [P] — each is a separate node file
- **Phase 5 (US3)**: T036, T038 are [P] — AST types and builtins are independent files
- **Phase 6 (US4)**: T044 is [P] with T043
- **Phase 10**: T053–T055 are [P] — each is a separate crate

---

## Parallel Example: Phase 2 (Foundational)

```
# All foundational tasks can run in parallel (different files):
Task T004: error.rs
Task T005: node.rs
Task T006: composition.rs
Task T007: graph.rs
Task T008: raster.rs
Task T009: surface_pool.rs
Task T010: sink.rs
Task T011: media.rs
Task T012: capability.rs
# Then sequential:
Task T013: render.rs (depends on T004-T012 types)
Task T014: lib.rs (depends on all modules existing)
```

## Parallel Example: Phase 3 (US1 Node Implementations)

```
# All source nodes can run in parallel:
Task T017: node/solid_color.rs
Task T018: node/shape.rs
Task T019: node/shape_renderer.rs
Task T020: node/text.rs
Task T021: node/media_output.rs

# Effect nodes in parallel:
Task T023: node/crop.rs
Task T024: node/resize.rs
Task T025: node/blur.rs
Task T026: node/shadow.rs

# Utility nodes in parallel:
Task T029: node/switch.rs
Task T030: node/frame_hold.rs
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (delete legacy, skeleton)
2. Complete Phase 2: Foundational (core types, graph, raster, render shell)
3. Complete Phase 3: US1 (all v1 nodes, graph optimization)
4. **STOP and VALIDATE**: Build SolidColor→Transform→Merge→MediaOutput graph, render, verify pixels
5. This is a working compositing engine with static rendering

### Incremental Delivery

1. Setup + Foundational → Core type system
2. US1 → Static rendering MVP
3. US2 → Animated rendering
4. US3 → Expression-driven animation
5. US4 → Video/image media integration
6. JSON → Serialization round-trip
7. US5 → Multithreaded export
8. US6 → Memo caching optimization
9. Dependent crates → Full workspace integration

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable after its dependencies
- Commit after each task or logical group
- No legacy code remains after Phase 1 — clean slate for all new implementation
- All node implementations follow the same pattern: struct in own file, `impl NodeEval`, wire into `NodeKind` enum match
- Expression parser adapted from existing code (not rewritten from scratch)
- FFmpeg adapter adapted from existing code (not rewritten from scratch)
