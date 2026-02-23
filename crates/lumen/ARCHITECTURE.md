# lumen/next Architecture

Follows a node-based style, similar to DaVinci Resolve Fusion. Nodes can have multiple inputs and should have one output, unless it is an edge node (e.g. composition output). Node outputs **must** be able to connect to multiple other node inputs.

This document defines the runtime architecture, graph model, rendering pipeline, platform integration boundaries, node system, and keyframe spec for `lumen/next`.

---

## Goals

* Fusion-style node graph for 2D compositing and motion graphics
* Skia-backed **CPU-first** raster rendering with pooled surfaces
* Deterministic frame-based rendering
* Platform-provided media implementations via stable traits
* Scalable architecture for multithreading on native platforms
* Animation via keyframes and expressions
* Portable core that compiles everywhere, with platform-specific adapters (including wasm)

---

## Core Principles

### 1) Core Compiles, Platforms Plug In

The core engine always retains the trait interfaces for media loading/decoding and output sinks.

* `MediaStore`
* `VideoFrameResolver`
* `ImageResolver`
* sink abstractions (encoder/canvas/etc.)

Features and platforms provide implementations. We do **not** compile placeholder behavior that returns runtime "feature unavailable" errors.

### 2) No Runtime "FeatureUnavailable" Errors

Unsupported capabilities should be prevented by:

* platform integration choices
* composition validation against the active runtime
* compile-time selection of adapters

The API should remain clean and trait-driven.

### 3) CPU-First Rendering (v1)

This version targets CPU raster rendering with Skia surfaces/bitmaps.

* No GPU rendering path in v1
* No GPU-specific raster union variants in v1
* All compositing and effects are CPU-oriented

### 4) Expressions Are Structured, Not Strings

Expressions are **not** string expressions in the runtime property model.

* Runtime stores parsed/typed expression structures
* The JSON delegate is responsible for converting serialized strings (or other JSON forms) into expression AST/delegates

---

## Composition Model

### Composition

A composition is the root renderable unit. It contains:

* Graph (nodes + connections)
* Timeline settings
* Render settings
* Animation tracks (keyframes)
* Optional metadata (editor/runtime integration)

### Timeline (Canonical Time Model)

Rendering is evaluated in **composition frames**.

* Canonical render unit: `frame: u32`
* Composition has:

  * `fps`
  * `duration_frames`
* Runtime may expose `time_seconds = frame as f64 / fps`

All time-based behavior (media playback, keyframes, expressions, range checks) is resolved from composition frame time.

### Stable Identity

We require stable IDs for deterministic evaluation, caching, and serialization.

* `NodeId` (stable, serialized)
* `TrackId` (stable, serialized)
* Ports addressed by:

  * named ports (preferred), or
  * indexed ports for dynamic-input nodes
* `EdgeId` may exist for editor/debug tooling

Node IDs must never depend on array position.

---

## Graph Model

### Nodes

A node is defined by:

* `id: NodeId`
* `kind`
* `properties`
* input bindings (connections and/or explicit defaults)
* optional metadata (editor-only)

### Connections

A connection links one node output to another node input.

* `from: (NodeId, OutputPort)`
* `to: (NodeId, InputPort)`

### Graph Validation (Required Before Rendering)

The graph must be validated before rendering:

* No cycles (feedback loops are not supported in v1)
* Port type compatibility
* Required inputs are connected (or node-defined default exists)
* Dynamic-input nodes satisfy their own invariants
* `Switch` input range mappings do not overlap
* A render request targets exactly one `MediaOutput` node

### Unconnected Inputs

Unconnected inputs are handled per-node and must be explicit in node definitions.

* Required input: validation error if missing
* Optional input: node-defined fallback behavior

No implicit "magic defaults" outside node specs.

---

## Render Evaluation Model

### Frame Evaluation

Rendering traverses the graph from `MediaOutput` at a specific composition frame.

* Input: `RenderContext`
* Output: sink-ready raster frame (usually bitmap-backed at sink boundary)

### RenderContext

The render context must include:

* `frame: u32`
* `fps`
* `width`
* `height`
* `duration_frames`
* references to caches/pools/stores
* active runtime capabilities/profile
* cancellation token (for long-running renders)

### Determinism

Node evaluation must be deterministic for a given:

* graph revision
* node properties
* animation tracks
* frame
* render settings
* runtime adapter behavior (for the same inputs)

This is required for caching, reproducibility, and multithreading correctness.

---

## CPU Raster Pipeline

## Color / Pixel Model (v1)

To avoid ambiguity and inconsistent blends, v1 uses a fixed internal render format.

* Color space: **sRGB**
* Pixel format: **RGBA8**
* Alpha: **Premultiplied alpha**

### Rules

* All sources are converted to the internal format on ingest/decode
* Blending and compositing assume premultiplied alpha
* Output sinks may convert formats at the final boundary if required (e.g. ffmpeg encoder path)

---

## RasterFrame Union Type

All raster node inputs/outputs use a dedicated union type (name fixed here as `RasterFrame`).

### `RasterFrame`

Represents CPU raster image data in one of two forms:

* `Bitmap`
* `Surface`

All prior references to `Bitmap | Surface` are now represented by `RasterFrame`.

### Responsibilities

`RasterFrame` must provide:

* dimensions/metadata access
* conversion to bitmap for sink/output
* promotion from bitmap to surface via `SurfacePool`
* ownership-safe movement through graph evaluation

### Promotion Rules

* Nodes that require mutable compositing/render targets promote incoming bitmap-backed data to a surface-backed `RasterFrame`
* Promotion uses `SurfacePool`
* Promotion may copy pixel data (this is acceptable in v1)

### Output Rule

`MediaOutput` must produce a sink-ready bitmap representation at the sink boundary, converting from surface-backed `RasterFrame` when necessary.

---

## SurfacePool

Manages pooled Skia surfaces for raster compositing.

* `SurfacePool`

  * `SurfaceRef`

    * Holds a pooled Skia Surface
    * On drop, clears and returns to the pool
  * Manual path (when needed)

    * `SurfacePool::acquire`
    * `SurfacePool::release`

### Required Behavior

* Pool keys must include dimensions and pixel format (and may include color space)
* If no compatible surface is available, **SurfacePool must create a new surface**
* SurfacePool is allowed to grow to satisfy peak concurrent node demand
* Returned surfaces are cleared before reuse (clear-on-release or clear-on-acquire; implementation must be consistent)

### Important Distinction

SurfacePool is **memory reuse**, not result caching.

---

## Media Integration (Trait-Driven)

The core engine defines media traits and uses them through dependency injection/runtime adapters.

### Core Traits (Retained Across Platforms)

* `MediaStore`
* `VideoFrameResolver`
* `ImageResolver`

These traits exist in the core regardless of target platform.

### Platform Implementations

Platforms provide concrete implementations.

Examples:

* Native runtime:

  * FFmpeg-backed video resolver
  * `image`-crate-backed image resolver
  * ffmpeg/codec sink (or file sink adapter)
* wasm runtime:

  * JS-backed media resolver (e.g. MediaBunny integration)
  * sink implemented as HTML canvas drawing

### Validation Against Runtime Capabilities

Composition validation must run against the active runtime profile/capability set.

Example:

* If a composition uses video nodes, the active runtime must provide a video resolver implementation
* If rendering to a canvas sink, `MediaOutput`/render session must target that sink type

This avoids runtime "FeatureUnavailable" behavior.

---

## Caching Strategy

Caching is split into layers.

### 1) Asset Cache (Shared)

Stores source-level data and resolver state.

Examples:

* image metadata by path/id
* decoded image cache by path/id
* video metadata by source/id
* resolver handles/state keyed by source identity

### 2) Node Output Cache (Per Render Session)

Caches evaluated node outputs for fan-out reuse.

Key dimensions must include:

* `node_id`
* `frame`
* render resolution
* graph revision
* property/animation revision hash

### 3) SurfacePool (Allocation Reuse)

Not a value cache.

### Invalidation Rules

At minimum:

* graph revision invalidates affected cached outputs
* node property changes invalidate affected cached outputs
* animation track changes invalidate affected cached outputs
* source asset changes invalidate associated asset cache entries (when hot reload is enabled)

---

## Graph Optimization (Required / Implemented Behavior)

Rendering begins by traversing from `MediaOutput` and pruning unused nodes.

### v1 Optimization Passes

These are part of the plan, not optional:

* **Dead node elimination**
* **Frame-range culling** (skip inactive nodes for current frame)
* **Per-frame fan-out memoization** (reuse node output when one output feeds multiple consumers)
* **Basic no-op elimination**

  * identity transform
  * transparent overlays / zero-opacity merges where behavior is unambiguous

### Debug Mode

A debug mode should disable optimization passes that obscure execution order, while preserving correctness.

---

## Multithreading (Native Runtime)

Multithreading is supported for native runtimes that provide the necessary threading primitives and media adapters. wasm is not the target for this path in v1.

### Overview

Multithreading has 4 required components:

1. **Orchestration**
2. **Media Sources**
3. **Rendering Workers**
4. **Media Sink**

### 1) Orchestration

Responsible for:

* splitting render into frame jobs
* scheduling jobs to workers
* bounded queueing/backpressure
* cancellation propagation
* error propagation

### 2) Media Sources

Responsible for:

* shared resolver access across worker threads
* synchronized decoder access
* request deduplication/coalescing where possible
* bounded request queues

### 3) Rendering Workers

Workers reuse the regular single-threaded renderer.

* receive frame jobs
* render frame through the standard graph evaluator
* emit frame results to sink queue

This keeps one rendering implementation and parallelizes at the frame-job level.

### 4) Media Sink

Responsible for:

* receiving rendered frames from workers
* ordering frames for sink submission
* writing to encoder/sink
* backpressure and cancellation handling

### Synchronization / Performance Guidance

* Use `parking_lot` for synchronization
* Use shared/global media store where appropriate
* Use bounded channels for orchestration and sink queues
* Video resolver state is synchronized (e.g. mutex-protected)
* Image resolver caches are shared and synchronized

### Ordered Output Contract

v1 requires ordered frame submission to the sink.
If workers complete out of order, the sink buffers/reorders before submission.

---

## Native Media Decode/Encode Strategy (FFmpeg-Style Adapter Guidance)

This section describes the expected strategy for native video adapters (e.g. ffmpeg-backed implementations).

### Decoder Thread + Sliding Window Strategy

For multithreaded rendering, the video resolver implementation should use a decoder thread and a sliding-window cache:

* decode `n` frames ahead of the current request region
* retain `n` frames behind to tolerate out-of-order worker progress
* discard frames outside the active window/cache policy

### Frame Request Flow

* Renderer requests `resolve_frame(frame)`
* If cached, return immediately
* If not cached, queue request to decoder and block/wait for completion
* Request coalescing should prevent duplicate decode work

### Reverse Playback

Reverse playback is supported with a correctness-first strategy in v1.

* A slower seek+decode-forward fallback is acceptable
* Behavior must be correct and deterministic
* Optimized GOP/chunk-aware reverse decode is post-v1 work

---

## Serialization and JSON Delegate

Serialization support is trait-/delegate-driven and preserves stable IDs and animation data.

### JSON Delegate Responsibilities

The JSON delegate is responsible for:

* serializing/deserializing graph + nodes + connections
* preserving stable IDs
* handling schema/version evolution
* converting serialized expression forms (e.g. strings) into parsed runtime expression representations

### Expression Parsing Boundary

Expression parsing belongs to the JSON delegate / import layer, **not** the runtime property model.

Runtime node properties store:

* literals, or
* typed expression objects/AST/delegates

Not raw expression strings.

---

## Node System

### Port Rules

Node outputs may connect to multiple node inputs.

### Port Types (v1)

#### Inputs

* `RasterFrame`
* `Surface`
* `Vector`

#### Outputs

* `RasterFrame`
* `Vector`

### Property Types (Runtime, v1)

* Expression (typed/parsed expression value, not string)
* Color
* String
* Int
* Float
* Boolean
* Map

`Map` exists as a property type but is not generally animatable in v1.

---

## Expressions

Expressions are evaluated at render time for a given frame using a typed expression representation.

### Expression Storage

* Runtime stores parsed/typed expressions
* JSON/import delegates convert serialized forms into expression objects

### Built-in Globals

* `frame`
* `time` (seconds)
* `fps`
* `width`
* `height`

### Built-in Math Functions

* `min`
* `max`
* `abs`
* `floor`
* `ceil`
* `round`
* `sin`
* `cos`
* `clamp`
* `lerp`
* `pow`
* `mod`
* `fract`
* `smoothstep`

### Text Functions

* `text_height(content, max_width, ...style_params)`
* `text_height(node_id)`

  * uses a Text node's styling/content to calculate height
* `text_width(content, max_width, ...style_params)`
* `text_width(node_id)`

  * uses a Text node's styling/content to calculate width
* `uppercase(content)`
* `lowercase(content)`

### Evaluation Precedence (Expressions vs Keyframes vs Static)

For a property at frame `F`:

1. If property has an expression, evaluate expression
2. Else if property has a keyframe track, sample the track
3. Else use static literal

This precedence is fixed for v1.

---

## Nodes (v1 Core Set)

This section includes the previously recommended utility nodes as part of the standard v1 node set.

---

### Shape

* Inputs

  * None
* Output

  * `Vector`
* Properties

  * Kind

    * Rectangle

      * Width
      * Height
    * Ellipse

      * Width
      * Height
    * Polygon

      * Point list

---

### ShapeRenderer

* Inputs

  * `Vector`
* Output

  * `RasterFrame`
* Properties

  * Fill Color
  * Stroke Width
  * Stroke Color
  * Fill Enabled
  * Stroke Enabled

---

### Media In

* Inputs

  * None
* Output

  * `RasterFrame`
* Properties

  * Kind

    * Image

      * Source
    * Video

      * Source
      * Range (composition-mapped source range)
      * Speed
      * Loop

### Media In Rules

* Source media is converted to internal RGBA8 premultiplied sRGB format
* Video frame lookup is resolved from composition frame time
* Speed modifies source-time mapping (time stretcher/remap node remains separate future work)

---

### Solid Color

* Inputs

  * None
* Output

  * `RasterFrame`
* Properties

  * Color
  * Width (defaults to composition width)
  * Height (defaults to composition height)

---

### Text

* Inputs

  * None
* Output

  * `RasterFrame`
* Properties

  * Content
  * Font Family
  * Font Size
  * Font Weight / Style
  * Max Width
  * Color
  * Alignment

### Text Rules

* Text metrics used by expression functions must match this node's layout behavior
* Text rendering and text metric calculation must share the same layout implementation

---

### Transform

* Inputs

  * `RasterFrame`
* Output

  * `RasterFrame`
* Properties

  * Scale X
  * Scale Y
  * Translate X
  * Translate Y
  * Rotate
  * Pivot X
  * Pivot Y

### Transform Rules

* Transform order is **Scale -> Rotate -> Translate**
* Coordinates are in composition pixel space
* Default sampling is bilinear
* Pivot defaults to the center of the input bounds when not explicitly set

---

### Crop

* Inputs

  * `RasterFrame`
* Output

  * `RasterFrame`
* Properties

  * X
  * Y
  * Width
  * Height

---

### Resize

* Inputs

  * `RasterFrame`
* Output

  * `RasterFrame`
* Properties

  * Width
  * Height
  * Mode (`Stretch`, `Fit`, `Fill`)
  * Sampling (`Nearest`, `Bilinear`)

---

### Blur

* Inputs

  * `RasterFrame`
* Output

  * `RasterFrame`
* Properties

  * Radius

---

### Shadow

* Inputs

  * `RasterFrame`
* Output

  * `RasterFrame`
* Properties

  * Color
  * Blur Radius
  * Offset X
  * Offset Y

### Shadow Rules

* Shadow is generated from the input alpha
* Shadow is composited with premultiplied alpha semantics
* Shadow output remains a `RasterFrame`

---

### Boolean (formerly Mask)

This node applies a raster/shape boolean mask operation to an input image.

* Inputs

  * Source (`RasterFrame`)
* Output

  * `RasterFrame`
* Properties

  * Kind

    * Shape

      * Rectangle

        * Width
        * Height
      * Ellipse

        * Width
        * Height
      * Polygon

        * Point list
    * Raster Mask

      * Source / reference (implementation-specific binding)
  * Invert (Boolean)

### Boolean Rules

* `Boolean` applies the generated/provided mask to the source input and outputs the masked result
* Mask interpretation is alpha-based
* Invert flips the mask before application
* Compositing is performed in premultiplied alpha space

---

### Merge

* Inputs

  * Base (`RasterFrame`)
  * Overlay (`RasterFrame`)
  * Mask (`RasterFrame`) optional
* Output

  * `RasterFrame`
* Properties

  * Blend Mode
  * Opacity

### Merge Rules

* Base is promoted to a surface-backed `RasterFrame` when mutable compositing is required
* Optional mask modulates overlay contribution
* Compositing uses premultiplied alpha semantics
* Opacity is applied to overlay contribution before final blend

---

### Switch

* Inputs

  * Dynamic inputs: `RasterFrame[]`
* Output

  * `RasterFrame`
* Properties

  * `Map<u16, Range<u32>>`

    * maps input index -> composition frame range
    * overlapping ranges are invalid

### Switch Rules

* Validation rejects overlapping ranges
* At a frame with no active range, output is transparent
* At a frame with one active range, that input is forwarded (promoted if needed for output consistency)

---

### Frame Hold

* Inputs

  * `RasterFrame`
* Output

  * `RasterFrame`
* Properties

  * Hold Frame (`u32` composition frame)

### Frame Hold Rules

* Samples upstream input at the specified composition frame regardless of current frame
* Useful for freeze frames and debugging timing

---

### Media Output

* Inputs

  * `RasterFrame`
* Output

  * Edge node (composition output)
* Properties

  * None (sink/session controls output behavior)

### Media Output Rules

* Final output is converted to sink-ready bitmap form at the sink boundary
* Native runtimes may encode to media sinks
* wasm runtimes may draw directly to canvas sinks

---

Perfect addition. A `Memo` node is a great fit for cross-session reuse, especially for expensive static subgraphs (text, shapes, repeated composites, etc.).

Here’s a solid v1-style node definition that matches your document style and constraints.

---

### Memo

Caches and reuses the rendered output of an upstream subgraph across render sessions when the subgraph is statically evaluable.

* Inputs

  * Source (`RasterFrame`)
* Output

  * `RasterFrame`
* Properties

  * Cache ID (`String`)

    * Stable identifier used to address memoized output in the persistent/shared cache store
  * Allow Expressions (`Boolean`)

    * Controls whether expression usage is permitted in the upstream subgraph for memoization eligibility

#### Memo Rules

* `Memo` evaluates and returns the upstream `Source` output like a pass-through node.
* `Memo` may persist and reuse the rendered output across render sessions using `Cache ID`.
* Cached outputs must be keyed by:

  * `Cache ID`
  * output dimensions
  * internal raster format (RGBA8 premultiplied sRGB)
  * a deterministic subgraph signature/hash (see below)
* `Memo` only reuses cached output when the upstream subgraph is considered **memoizable** under the rules below.

#### Memoization Eligibility (v1)

A `Memo` node may cache/reuse output only if all of the following are true:

1. **Upstream subgraph is deterministic**

   * No non-deterministic node behavior (unless Allow Expressions = true)
   * No runtime-dependent side effects
   * No platform-variant behavior that would change output for the same inputs (within the active runtime profile)

2. **Upstream subgraph is frame-static for the requested render**

   * Output does not depend on composition frame/time
   * No time-varying media sampling (unless reduced to a static frame by upstream nodes such as `Frame Hold` in a statically provable way)

3. **Expression policy passes**

   * If `Allow Expressions = false`:

     * Expressions are allowed **only if they are statically evaluable**
     * If any expression depends on runtime values (e.g. `frame`, `time`, dynamic node metrics that vary with animated inputs, or other non-static context), the memo is ineligible and must re-render (pass-through only, no cache write)
   * If `Allow Expressions = true`:

     * It will always cache, and may result in undefined behavior at times.

4. **Subgraph signature matches**

   * A deterministic signature/hash of the upstream subgraph (node types, relevant properties, static expression results/AST, and topology) must match the cached entry

#### Cache Write / Read Behavior

* On render:

  1. Analyze upstream subgraph for memoization eligibility
  2. If eligible, compute memo cache key/signature
  3. Attempt cache read using `Cache ID` + signature + output metadata
  4. If hit: return cached `RasterFrame` (bitmap-backed is recommended for persistence)
  5. If miss: render source, persist result, return rendered result
* Persisted memo entries should store bitmap form (not pooled surface handles)

#### Output Type and Persistence Rule

* `Memo` outputs `RasterFrame`
* Persisted cache representation must be bitmap-backed (or an equivalent serialized raster representation)
* Surface-backed results must be converted before persistence

#### Validation Rules

* `Cache ID` must be non-empty
* `Cache ID` should be treated as a user namespace key (collisions are allowed but produce shared cache behavior by design)
* If multiple `Memo` nodes share the same `Cache ID`, correctness still depends on subgraph signature matching (i.e. `Cache ID` alone is not sufficient)

#### Notes

* `Memo` is intended for cross-session reuse of expensive static subgraphs.
* `Memo` is distinct from per-session node output caching:

  * per-session cache = automatic runtime optimization
  * `Memo` = user-authored persistent cache boundary
* `Memo` should be placed at stable subgraph boundaries (e.g. generated backgrounds, static text blocks, static shape composites).

---

## Error Handling and Diagnostics

The engine uses structured errors and warnings. These are **not** capability/feature toggle errors.

### Error Categories

* `GraphValidationError`
* `PropertyError`
* `ExpressionError`
* `MediaError`
* `RenderError`
* `ThreadingError`
* `SinkError`

### Diagnostics Requirements

Errors and warnings should include context where possible:

* `node_id`
* node kind
* property path/name
* frame
* source identifier/path
* sink context (when relevant)

### Warnings (Non-fatal)

Examples:

* source range clamped to duration
* source fps mismatch with composition fps
* optional input default applied
* sink format conversion applied at output boundary

---

## Keyframes / Animation Spec (v1)

Animation is represented as keyframe tracks targeting node property paths.

### Property Value Sources

A property may be:

* static literal
* keyframed
* expression-driven

Precedence is fixed (expression > keyframe > static).

---

### Track Model

A keyframe track targets exactly one property on exactly one node.

* `track_id`
* `node_id`
* `property_path`
* `value_type`
* `keys`
* `before_extrapolation`
* `after_extrapolation`

### Property Path

Stable property paths are required, e.g.

* `transform.translate_x`
* `transform.translate_y`
* `transform.rotate`
* `merge.opacity`
* `shape.kind.rectangle.width`

Paths must be stable across serialization and validation.

---

### Time Representation

Keyframes are stored in composition frames.

* `time_frame: u32`

Rules:

* keys sorted ascending
* duplicate `time_frame` in same track is invalid

---

### Animatable Value Types (v1)

Supported:

* Float
* Int
* Boolean
* Color
* Vector2
* String (step only)
* Enum-like values (step only)

Deferred:

* `Map`
* polygon point-list animation
* source-path animation

---

### Interpolation Modes (v1)

Implemented:

* `Step`
* `Linear`

Type rules:

* Float: Step / Linear
* Int: Step (Linear allowed only if explicit rounded semantics are implemented; otherwise Step only)
* Boolean: Step only
* Color: Step / Linear (RGBA in internal color model)
* String / Enum: Step only

---

### Extrapolation

Each track defines:

* `before`: `Hold | DefaultValue`
* `after`: `Hold | DefaultValue`

Default is:

* `Hold` before
* `Hold` after

---

### Sampling Rules

To sample a track at frame `F`:

* no keys -> invalid track
* one key -> return that value
* before first key -> apply `before` extrapolation
* after last key -> apply `after` extrapolation
* otherwise:

  * binary search adjacent keys
  * compute interpolation factor
  * interpolate by mode and type

---

### Track Validation

Validation must ensure:

* target node exists
* property path exists on that node kind
* value type matches property type
* keys are sorted and unique
* interpolation mode is valid for the value type
* only one track per `(node_id, property_path)` in v1

---

### Runtime/Editor Operations (Required API Surface)

Provide helper operations:

* `set_key(node_id, property_path, frame, value)`
* `remove_key(track_id, frame)`
* `sample_property(node_id, property_path, frame)`
* `list_animatable_properties(node_id)`

---

## Runtime Capability Profiles (Validation Target)

To support multiple platforms cleanly, rendering should validate against a runtime capability profile.

### Capability Profile Must Describe

* available media resolver types (image/video)
* available sink types (encoder/file/canvas/etc.)
* threading support
* platform limits (if any)

### Purpose

This profile is used during validation/session setup so the composition is accepted or rejected before rendering begins.

This replaces runtime "feature unavailable" behavior with upfront compatibility checks.

---

## Implementation Checklist (Locked-In Decisions)

* [x] Canonical time unit is composition frame (`u32`)
* [x] Stable node and track IDs are required
* [x] Graph validation is required before render
* [x] No cycles in v1
* [x] CPU-only raster rendering in v1
* [x] Internal format is RGBA8 premultiplied sRGB
* [x] All raster ports use `RasterFrame`
* [x] `SurfacePool` grows by allocating new surfaces when pool is exhausted
* [x] Expressions are typed runtime values, not raw strings
* [x] JSON delegate handles expression parsing/import conversion
* [x] No `FeatureUnavailable` runtime error path
* [x] Platform/runtime adapters provide trait implementations
* [x] `Boolean` node replaces `Mask` and applies a mask to a source `RasterFrame`
* [x] Ordered sink submission in multithreaded native rendering

---

## Future Work / Post-v1 (Retained)

### Rendering / Performance

* More aggressive graph optimization and subgraph memoization across frames
* Tile-based rendering for very large compositions
* Motion blur / subframe sampling
* Better dependency-hash-based invalidation
* SIMD-optimized CPU effects where beneficial

### Color Management

* Wide-gamut / HDR support
* Explicit color transforms per source
* Linear-light compositing options

### Nodes / Features

* Time Remap node (reverse, ramps, arbitrary remap curves)
* Color correction nodes (levels, curves, exposure, saturation, hue)
* Advanced text layout/render split
* Multi-merge and layer-stack helpers
* Expanded boolean/mask operations and feathering
* Audio graph/domain support

### Animation

* Bezier keyframes with tangent handles
* Curve editor metadata
* Expression access to base animated value (`value`)
* Additive/layered animation tracks
* Polygon/path point animation
* Subframe keyframes (rational time)

### Media / Multithreading

* Smarter reverse video decode (GOP/chunk caches)
* Advanced decode request prioritization/coalescing strategies
* wasm threading path if target/runtime support becomes practical

### Serialization / Project Format

* Versioned schema migrations
* Stable editor metadata schema
* Asset relinking and path remapping
* Project packaging/bundling format
