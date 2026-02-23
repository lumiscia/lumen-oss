# Feature Specification: Lumen/Next Compositing Engine

**Feature Branch**: `002-lumen-next-engine`
**Created**: 2026-02-23
**Status**: Draft
**Input**: User description: "Implement the lumen/next compositing engine — a Fusion-style node graph for 2D compositing and motion graphics with CPU-first Skia rendering, trait-driven media integration, keyframe animation, and cross-platform support (native + wasm)."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Render a Static Composition (Priority: P1)

A developer defines a node graph containing source media (images, solid colors, shapes, text), applies transforms and blending via Merge nodes, and renders a single frame to a bitmap output through a MediaOutput node.

**Why this priority**: Core graph evaluation and raster rendering is the foundational capability. Every other feature builds on the ability to evaluate a valid graph at a given frame and produce a correct raster output.

**Independent Test**: Can be fully tested by constructing a minimal graph (e.g., SolidColor → Transform → MediaOutput), rendering frame 0, and verifying the output bitmap matches expected pixel data.

**Acceptance Scenarios**:

1. **Given** a valid graph with a SolidColor node connected through a Transform to a MediaOutput node, **When** rendering frame 0 at 1920x1080, **Then** the engine produces a correctly transformed RGBA8 premultiplied sRGB bitmap.
2. **Given** a graph with a Merge node compositing two inputs with blend mode Normal at opacity 0.5, **When** rendering any frame, **Then** the overlay contribution is modulated by 0.5 opacity before final blend using premultiplied alpha semantics.
3. **Given** a graph with an unconnected required input, **When** validation runs before rendering, **Then** the engine rejects the graph with a structured error identifying the missing input, node ID, and node kind.

---

### User Story 2 - Animate Properties with Keyframes (Priority: P1)

A developer defines keyframe tracks on node properties (e.g., Transform translate_x, Merge opacity) and renders a sequence of frames. The engine interpolates property values correctly at each composition frame.

**Why this priority**: Animation is a core differentiator for a motion graphics engine. Without keyframe evaluation, the engine is limited to static compositing.

**Independent Test**: Can be tested by creating a Transform node with a linear keyframe track on translate_x (frame 0 → 0px, frame 30 → 100px), rendering frames 0, 15, and 30, and verifying the transform position at each frame.

**Acceptance Scenarios**:

1. **Given** a keyframe track with Linear interpolation on a Float property with keys at frame 0 (value 0.0) and frame 60 (value 100.0), **When** sampling at frame 30, **Then** the engine returns 50.0.
2. **Given** a keyframe track with Step interpolation on a Boolean property, **When** sampling between keys, **Then** the engine holds the previous key's value until the next key frame.
3. **Given** a keyframe track with Hold extrapolation before and after, **When** sampling before the first key or after the last key, **Then** the engine returns the nearest key value.
4. **Given** a property with both an expression and a keyframe track, **When** evaluating at any frame, **Then** the expression takes precedence over the keyframe.

---

### User Story 3 - Evaluate Expressions at Render Time (Priority: P2)

A developer assigns typed expressions to node properties that reference built-in globals (frame, time, fps, width, height) and math functions. The engine evaluates these expressions at each frame during rendering.

**Why this priority**: Expressions enable dynamic, procedural animation that keyframes alone cannot achieve (e.g., wiggle, responsive sizing, time-based logic).

**Independent Test**: Can be tested by assigning an expression using sin and time globals to a Transform's translate_y property, rendering multiple frames, and verifying the output matches expected sine wave positions.

**Acceptance Scenarios**:

1. **Given** an expression referencing `frame` and `fps`, **When** rendering frame 30 at 30fps, **Then** `frame` evaluates to 30 and `time` evaluates to 1.0.
2. **Given** an expression using lerp with frame-based interpolation, **When** rendering frame 30 of a 60-frame composition, **Then** the result matches the expected interpolated value.
3. **Given** an expression referencing an undefined variable, **When** validation runs, **Then** the engine reports a structured expression error with the node ID and property path.

---

### User Story 4 - Decode and Composite Video Media (Priority: P2)

A developer adds a MediaIn node with a video source, maps a source range to composition time, and composites it with other layers. The engine resolves video frames through the platform-provided VideoFrameResolver trait.

**Why this priority**: Video compositing is a primary use case for a motion graphics tool, but depends on the core graph and rendering pipeline being functional first.

**Independent Test**: Can be tested by providing a mock VideoFrameResolver that returns known test frames, creating a MediaIn(Video) → MediaOutput graph, and verifying that the correct source frame is resolved for each composition frame.

**Acceptance Scenarios**:

1. **Given** a MediaIn(Video) node with speed 1.0 and a mapped source range, **When** rendering composition frame F, **Then** the engine requests the correct source frame from the VideoFrameResolver.
2. **Given** a MediaIn(Video) node with speed 2.0, **When** rendering, **Then** source time advances at twice the composition rate.
3. **Given** a composition that uses video nodes but the active runtime provides no VideoFrameResolver, **When** validation runs against the runtime capability profile, **Then** the composition is rejected before rendering begins.

---

### User Story 5 - Render a Frame Sequence with Multithreading (Priority: P3)

A developer triggers a multi-frame render (e.g., frames 0–300) on a native platform. The engine distributes frame jobs across worker threads, and the sink receives frames in correct order.

**Why this priority**: Multithreaded rendering is critical for native performance but builds on single-threaded correctness. It is a platform-specific optimization, not a core correctness requirement.

**Independent Test**: Can be tested by rendering a 60-frame composition with 4 worker threads, verifying all 60 frames arrive at the sink in order, and confirming pixel-identical output to single-threaded rendering.

**Acceptance Scenarios**:

1. **Given** a render job for frames 0–59 with 4 workers, **When** workers complete frames out of order, **Then** the sink receives and writes all frames in ascending frame order.
2. **Given** a cancellation signal during a multi-frame render, **When** the orchestrator propagates cancellation, **Then** all workers stop, in-progress frames are discarded, and the sink is finalized cleanly.
3. **Given** a render error on one frame, **When** the error propagates, **Then** the orchestrator reports the error with frame number, node ID, and node kind context.

---

### User Story 6 - Use Memo Nodes for Cross-Session Caching (Priority: P3)

A developer places Memo nodes at stable subgraph boundaries (e.g., a static background composite). On subsequent render sessions, the engine reuses the cached bitmap output instead of re-rendering the static subgraph.

**Why this priority**: Memo is a performance optimization for expensive static subgraphs. It enhances workflow efficiency but is not required for correctness.

**Independent Test**: Can be tested by rendering a graph with a Memo node, modifying an unrelated node, re-rendering, and verifying the memoized subgraph was not re-evaluated.

**Acceptance Scenarios**:

1. **Given** a Memo node with a frame-static upstream subgraph and a valid Cache ID, **When** rendering for the first time, **Then** the engine renders the subgraph and persists the result as a bitmap.
2. **Given** a Memo node with a previously cached result and an unchanged subgraph signature, **When** rendering again, **Then** the engine returns the cached bitmap without re-evaluating the upstream subgraph.
3. **Given** a Memo node whose upstream subgraph depends on frame/time and Allow Expressions is false, **When** eligibility is checked, **Then** the memo is ineligible and the subgraph is rendered fresh without cache write.

---

### Edge Cases

- What happens when a graph contains a cycle? The engine must detect cycles during validation and reject the graph with a structured error before rendering begins.
- What happens when a Switch node has overlapping frame ranges? Validation must reject the graph, identifying the specific overlapping ranges and node ID.
- What happens when the surface pool is exhausted at peak demand? The pool must create new surfaces to satisfy demand — it grows, never fails.
- What happens when a video source has a different FPS than the composition? The engine issues a non-fatal warning about the FPS mismatch and resolves frames using composition frame time mapping.
- What happens when a keyframe track has duplicate time_frame values? Validation must reject the track as invalid.
- What happens when a Memo node's Cache ID is empty? Validation must reject the node.
- What happens during reverse video playback? The engine uses a correctness-first seek+decode-forward strategy, producing deterministic output even if slower.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST implement a directed acyclic node graph where nodes have typed input/output ports and connections link one node's output port to another node's input port.
- **FR-002**: System MUST validate the graph before rendering, checking for: cycles, port type compatibility, required input connectivity, dynamic-input node invariants, Switch range overlap, and exactly one MediaOutput target.
- **FR-003**: System MUST evaluate the graph by traversal from MediaOutput at a specific composition frame, producing a sink-ready RGBA8 premultiplied sRGB bitmap.
- **FR-004**: System MUST support the v1 node set: Shape, ShapeRenderer, MediaIn (Image/Video), SolidColor, Text, Transform, Crop, Resize, Blur, Shadow, Boolean, Merge, Switch, FrameHold, MediaOutput, and Memo.
- **FR-005**: System MUST use composition frames as the canonical time unit, with timeline settings (fps, duration_frames) driving all time-based behavior.
- **FR-006**: System MUST support keyframe animation tracks targeting node property paths, with Step and Linear interpolation, and Hold/DefaultValue extrapolation.
- **FR-007**: System MUST evaluate property values with fixed precedence: expression > keyframe > static literal.
- **FR-008**: System MUST support typed expressions with built-in globals (frame, time, fps, width, height) and built-in math functions (min, max, abs, floor, ceil, round, sin, cos, clamp, lerp, pow, mod, fract, smoothstep) and text functions (text_height, text_width, uppercase, lowercase).
- **FR-009**: System MUST use stable IDs (NodeId, TrackId) that never depend on array position, enabling deterministic evaluation, caching, and serialization.
- **FR-010**: System MUST implement a surface pool that manages pooled raster surfaces, grows by allocating new surfaces when exhausted, and clears surfaces on reuse.
- **FR-011**: System MUST implement graph optimization passes: dead node elimination, frame-range culling, per-frame fan-out memoization, and basic no-op elimination.
- **FR-012**: System MUST support Merge node compositing with premultiplied alpha semantics, blend modes, opacity modulation, and optional mask input.
- **FR-013**: System MUST support media integration through trait interfaces (MediaStore, VideoFrameResolver, ImageResolver) with platform-provided implementations.
- **FR-014**: System MUST validate compositions against the active runtime capability profile, rejecting compositions that require unavailable capabilities before rendering begins.
- **FR-015**: System MUST support multithreaded frame-parallel rendering on native platforms with orchestration, bounded queueing, cancellation propagation, and ordered sink submission.
- **FR-016**: System MUST serialize/deserialize graphs via a JSON delegate that preserves stable IDs, handles schema evolution, and converts serialized expression forms into typed runtime representations.
- **FR-017**: System MUST implement three cache layers: asset cache (shared, source-level), node output cache (per render session, fan-out reuse), and surface pool (allocation reuse only).
- **FR-018**: System MUST support the Memo node for cross-session caching of eligible static subgraphs, keyed by Cache ID, dimensions, format, and subgraph signature hash.

### Security and Boundary Requirements *(mandatory)*

- **SR-001**: System MUST define trust boundaries at: media file I/O (image/video sources), platform trait adapters (VideoFrameResolver, ImageResolver), sink output (encoder/canvas), and JSON deserialization input.
- **SR-002**: System MUST validate all external media source paths/references during composition validation before rendering. Malformed or inaccessible sources must produce a structured error with source context, not a crash.
- **SR-003**: System MUST validate JSON input during deserialization — malformed, cyclic, or schema-violating input must be rejected with structured diagnostics, never silently accepted.
- **SR-004**: System MUST ensure expression evaluation is sandboxed to built-in globals and functions only — no arbitrary code execution or system access through expressions.

### Operational Requirements *(mandatory)*

- **OR-001**: System MUST produce structured errors with context (node_id, node kind, property path, frame, source identifier) for all error categories: graph validation, property, expression, media, render, threading, and sink errors.
- **OR-002**: System MUST produce non-fatal warnings for recoverable conditions: source range clamped, FPS mismatch, optional input default applied, sink format conversion applied.
- **OR-003**: System MUST support cancellation tokens for long-running renders, propagating cancellation through the orchestrator to workers and cleaning up in-progress work.
- **OR-004**: System MUST implement bounded channels for multithreaded orchestration and sink queues to prevent unbounded memory growth.

### Key Entities

- **Composition**: Root renderable unit containing graph, timeline settings, render settings, animation tracks, and metadata.
- **Node**: Graph element with a stable NodeId, kind, properties, and typed input/output port bindings. Defined by the v1 node set.
- **Connection**: Directed link from one node's output port to another node's input port, forming the evaluation graph.
- **KeyframeTrack**: Animation data targeting a specific (node_id, property_path) pair, containing sorted keys with interpolation modes and extrapolation settings.
- **RasterFrame**: Union type representing CPU raster image data as either Bitmap or Surface, used as the primary port type for raster data flow.
- **Expression**: Typed/parsed expression value stored in the runtime property model, evaluated at render time against built-in globals and functions.
- **RenderContext**: Per-frame evaluation context carrying frame, fps, dimensions, duration, cache/pool references, runtime capabilities, and cancellation token.
- **RuntimeCapabilityProfile**: Descriptor of available media resolvers, sink types, threading support, and platform limits — used for upfront validation.
- **Memo**: Persistent cache boundary node that stores and reuses rendered output of eligible static subgraphs across render sessions.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A valid composition graph with up to 50 nodes renders a single frame correctly and deterministically — identical input always produces identical output.
- **SC-002**: Keyframe-animated properties produce frame-accurate interpolated values with less than 0.001 error for floating-point linear interpolation.
- **SC-003**: Graph validation catches 100% of structural errors (cycles, missing required inputs, type mismatches, overlapping Switch ranges) before rendering begins.
- **SC-004**: Multi-frame renders on native platforms achieve near-linear speedup with additional worker threads (e.g., 4 workers deliver at least 3x throughput vs. single-threaded for CPU-bound compositions).
- **SC-005**: Memo nodes with eligible static subgraphs achieve cache hit on re-render without subgraph changes, reducing repeated render time for memoized subgraphs to near-zero.
- **SC-006**: The engine runs on both native and wasm platforms using the same core graph evaluation logic, differing only in platform-provided trait implementations.
- **SC-007**: All structured errors include sufficient context (node ID, node kind, frame number, property path) for a developer to locate and fix the issue without debugging the engine internals.
- **SC-008**: Expressions evaluate correctly for all built-in globals and math functions, with expression errors reported at validation time when statically detectable.

## Assumptions

- The raster backend for v1 is CPU-based using Skia surfaces and bitmaps.
- wasm targets use single-threaded rendering in v1 (no wasm threading path).
- The internal pixel format (RGBA8 premultiplied sRGB) is fixed for v1 — no wide-gamut or HDR support.
- GPU rendering is explicitly out of scope for v1.
- Bezier keyframe interpolation is deferred to post-v1.
- Audio is out of scope for v1.
- The JSON delegate handles all serialization concerns including schema versioning and expression parsing.
- Platform adapters are provided by the host runtime, not by the core engine.
