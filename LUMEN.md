# Lumen — Architecture & Rewrite Guide

> **This is a ground-up rewrite.** Ignore the existing code unless you are stuck on a specific
> implementation detail, in which case reference it narrowly. Do not port patterns, module
> structures, or naming from the old codebase without evaluating them against this document first.

---

## Table of Contents

1. [Overview](#overview)
2. [Crate Structure](#crate-structure)
3. [Feature Set](#feature-set)
4. [Style Values & Expressions](#style-values--expressions)
5. [Data Model & JSON Schema](#data-model--json-schema)
6. [Styling System](#styling-system)
7. [Expression System](#expression-system)
8. [Compilation Pipeline](#compilation-pipeline)
9. [Rendering Architecture](#rendering-architecture)
10. [Multithreaded Rendering](#multithreaded-rendering)
11. [Video Frame Handling](#video-frame-handling)
12. [Rust Practices](#rust-practices)

---

## Overview

Lumen is a compositing and rendering engine for timeline-based media projects. A **project** defines
a canvas, a timeline (fps + duration), media sources, and a layer stack of clips. Projects are
serialized as JSON and compiled into an intermediate representation that the renderer consumes
frame-by-frame across multiple threads.

The design priorities are:

- **Determinism** — identical input always produces identical output.
- **Memory efficiency** — video frames are never fully buffered; they are fetched on demand.
- **Expressiveness** — every numeric style property is either a literal value or a dynamic
  expression. Expressions replace a dedicated animation system entirely.
- **Performance** — frames are rendered in parallel across a thread pool.
- **Backend agnosticism** — the `lumen` crate has no knowledge of FFmpeg, mediabunny, or any
  specific decoder/encoder. It owns compilation, expression evaluation, and rendering only.

---

## Crate Structure

```
crates/
  lumen/          # Core: model, expressions, compilation, Skia renderer
  lumen-server/   # HTTP server + FFmpeg decode + job queue
  lumen-wasm/     # WASM/FFI layer for web preview
  lumen-local/    # CLI binary for local rendering and testing
```

**Boundary rules:**

- `lumen` must not depend on `ffmpeg-next`, `mediabunny`, `axum`, `tokio`, or any async runtime.
- `lumen` depends on `skia-safe`, `taffy`, `serde`, `thiserror`, `image`, and `rayon`.
- Video decoding, network fetching, and encoding live exclusively in `lumen-server` or platform
  crates.
- `lumen-wasm` and `lumen-local` are integration shims — they implement the `FrameProvider` trait
  using platform-specific decoders and pass it to the render orchestrator.

---

## Feature Set

### Clips

Every clip occupies a time range on a layer and carries a style. Supported types:

| Type     | Description                                                                  |
|----------|------------------------------------------------------------------------------|
| `solid`  | Filled rectangle; useful as a background or color card                       |
| `shape`  | Vector shape: rectangle, ellipse, or polygon with optional bezier curves     |
| `text`   | Styled text block                                                            |
| `image`  | Single frame from an image source                                            |
| `video`  | Frame sequence from a video source with an optional source pipeline          |
| `group`  | Container that composites children before blending into the layer            |
| `layout` | Flexbox container (taffy) with nested layout nodes                           |

### Shapes

- **Rectangle** — optional per-corner radii `[tl, tr, br, bl]`
- **Ellipse** — parameterized by its bounding box
- **Polygon** — list of vertices each optionally accompanied by cubic bezier control handles,
  enabling smooth curves

### Masks

Every clip and group may have exactly one mask. A mask is itself a full `LayerItem` (any clip type,
including a group). The mask's **alpha channel** determines visibility of the masked content.

- Masks support full styling (opacity, transform, blur) but do not composite into the layer.
- Simple cases (rect/ellipse mask geometry) use Skia clip operations — zero offscreen allocation.
- Complex cases render to an offscreen surface and apply it as an alpha map via `DstIn`.

### Layouts

A `layout` clip contains a tree of layout nodes backed by `taffy`. Every numeric node property
(width, height, padding, gap, etc.) is a `StyleValue` and participates in the expression system
using the node's `id` as the reference target.

Layout is computed **before** per-frame expression evaluation so that resolved node sizes are
available to expressions that reference `<layout_node_id>.width` etc. Circular dependencies between
layout node properties and expressions that reference those nodes are detected at compile time and
rejected with `CompileError::CircularDependency`.

---

## Style Values & Expressions

There is no separate animation object. Every numeric style property is a `StyleValue`:

```rust
pub enum StyleValue {
    Value(f32),
    Expr(String),
}
```

In JSON, a property is written as a number (literal) or a string (expression):

```json
"opacity": 1.0
"opacity": "clamp(timeline.frame / 30.0, 0.0, 1.0)"
"x": "canvas.width * 0.5"
"font_size": "32 + clip_title.height * 0.1"
```

Expressions are the animation system. Any property that varies over time uses `timeline.frame` in
its expression. This is deliberately more explicit than keyframe tracks but removes the need for
parallel systems — one mechanism drives both static values and animated ones.

For convenience, expressions support built-in math functions:

| Function | Description |
|---|---|
| `clamp(x, min, max)` | Clamps `x` to `[min, max]` |
| `min(a, b)` / `max(a, b)` | Minimum / maximum |
| `abs(x)` | Absolute value |
| `floor(x)` / `ceil(x)` / `round(x)` | Rounding |
| `sqrt(x)` | Square root |
| `sin(x)` / `cos(x)` | Trigonometry (radians) |
| `mix(a, b, t)` | Linear interpolation: `a + (b - a) * t` |
| `step(edge, x)` | Returns `0` if `x < edge`, else `1` |
| `smoothstep(e0, e1, x)` | Smooth Hermite interpolation |

---

## Data Model & JSON Schema

### Project

```json
{
  "version": "1",
  "canvas": {
    "width": 1920,
    "height": 1080,
    "background": [0, 0, 0, 255]
  },
  "timeline": {
    "fps": [30, 1],
    "duration_frames": 300
  },
  "sources": [],
  "layers": [],
  "audio": { "tracks": [] }
}
```

### Source

```json
{ "id": "src_video", "media": "video", "kind": { "type": "file", "path": "./clip.mp4" } }
{ "id": "src_logo",  "media": "image", "kind": { "type": "url",  "url": "https://..." } }
```

`kind.type`: `file` | `url`. URLs are downloaded and validated before compilation.
`media`: `video` | `image` | `audio`.

### Layer

```json
{ "id": "layer_0", "items": [] }
```

### LayerItem

Discriminated by `"type"`. Valid types: `"clip"` | `"group"`.

```json
{
  "type": "clip",
  "id": "clip_bg",
  "start_frame": 0,
  "duration_frames": 300,
  "content": { },
  "style": { },
  "mask": null
}
```

```json
{
  "type": "group",
  "id": "grp_intro",
  "items": [],
  "style": { },
  "mask": null
}
```

### ClipContent variants

**Solid**
```json
{ "type": "solid" }
```
Color comes entirely from style (`fill`).

**Shape**
```json
{
  "type": "shape",
  "geometry": { "kind": "rect" }
}
```
```json
{
  "type": "shape",
  "geometry": { "kind": "ellipse" }
}
```
```json
{
  "type": "shape",
  "geometry": {
    "kind": "polygon",
    "vertices": [
      { "x": 0,   "y": 0,   "cp_in": null,       "cp_out": [10, -20] },
      { "x": 100, "y": 0,   "cp_in": [-10, -20],  "cp_out": null     },
      { "x": 50,  "y": 100, "cp_in": null,        "cp_out": null     }
    ],
    "closed": true
  }
}
```

Fill, stroke, and corner radii come from style.

**Text**
```json
{
  "type": "text",
  "content": "Hello, world"
}
```

`content` is a plain `String`, not an expression. All text styling (font, size, color, alignment)
lives in the clip's style.

**Image**
```json
{ "type": "image", "source": "src_logo" }
```

**Video**
```json
{
  "type": "video",
  "source": "src_video",
  "pipeline": {
    "trim": { "start_frame": 0, "end_frame": 150 },
    "speed": 1.0,
    "loop": "none"
  }
}
```

`speed` may be negative to play in reverse. `loop` values: `"none"` | `{ "finite": 3 }` |
`"infinite"`. The pipeline is not part of style — it describes source frame mapping, not
appearance.

**Layout**
```json
{
  "type": "layout",
  "root": { }
}
```

---

## Styling System

Every clip type shares a common base style. Clip-type-specific properties are added alongside the
base fields in the same `style` object. All numeric fields are `StyleValue` (literal or expression).
Every property listed here is independently animatable by writing an expression instead of a number.

### Base Style (all clip types and groups)

```json
{
  "visible": true,
  "opacity": 1.0,
  "blend_mode": "normal",
  "blur": 0.0,
  "shadow": {
    "offset_x": 4.0,
    "offset_y": 4.0,
    "blur": 12.0,
    "color": [0, 0, 0, 128]
  },
  "transform": {
    "x": 0,
    "y": 0,
    "width": "canvas.width",
    "height": "canvas.height",
    "rotation": 0.0,
    "anchor_x": 0.5,
    "anchor_y": 0.5,
    "scale_x": 1.0,
    "scale_y": 1.0,
    "skew_x": 0.0,
    "skew_y": 0.0
  },
  "alignment": [0, 0]
}
```

**`alignment`** — a two-element array `[x, y]` where each component is a `StyleValue` in `[-1,
1]`. Controls where `(transform.x, transform.y)` is anchored relative to the clip's own bounds:

| Value | Meaning |
|---|---|
| `[-1, -1]` | Top-left corner at `(x, y)` |
| `[0, 0]` | Center at `(x, y)` *(default)* |
| `[1, 1]` | Bottom-right corner at `(x, y)` |

**`anchor_x` / `anchor_y`** — 0–1 fractions of the clip's own width/height. Controls the pivot
point for rotation and scale. Default `0.5` (center). Distinct from `alignment`: anchor affects
rotation/scale origin, alignment affects positional offset.

**`blend_mode`**: `normal | multiply | screen | overlay | darken | lighten | color_dodge |
color_burn | hard_light | soft_light | difference | exclusion | hue | saturation | color |
luminosity`

**`visible`** — boolean on/off, distinct from `opacity: 0` (a hidden clip does not participate in
masking or expression context).

### Shape-specific style

```json
{
  "fill": [255, 0, 0, 255],
  "stroke": {
    "color": [0, 0, 0, 255],
    "width": 2.0,
    "dash": { "pattern": [8, 4], "offset": 0.0 }
  },
  "corner_radius": [8, 8, 8, 8]
}
```

`fill` and `stroke.color` are RGBA `[u8; 4]`. `stroke.width`, `stroke.dash.offset`, and each
element of `corner_radius` are `StyleValue`. `stroke.dash` is optional; omit for a solid stroke.

### Text-specific style

```json
{
  "font_family": "Inter",
  "font_size": 48.0,
  "font_weight": 400,
  "color": [255, 255, 255, 255],
  "align": "center",
  "vertical_align": "middle",
  "letter_spacing": 0.0,
  "line_height": 1.2
}
```

`font_size`, `font_weight`, `letter_spacing`, and `line_height` are `StyleValue`.
`align`: `left | center | right`. `vertical_align`: `top | middle | bottom`.
`font_family` and enum fields are plain strings — not expressions.

### Image / Video-specific style

```json
{
  "fit": "cover",
  "color_matrix": null
}
```

`fit`: `cover | contain | fill | none`.
`color_matrix`: optional 4×5 `[[f32; 5]; 4]` RGBA color transform (brightness, contrast,
saturation, hue, tint). Applied as a Skia color filter.

### Complete style example (animated fade-in video)

```json
{
  "type": "clip",
  "id": "clip_intro",
  "start_frame": 0,
  "duration_frames": 90,
  "content": { "type": "video", "source": "src_main", "pipeline": { "speed": 1.0, "loop": "none" } },
  "style": {
    "opacity": "smoothstep(0.0, 30.0, timeline.frame)",
    "blend_mode": "normal",
    "blur": 0.0,
    "visible": true,
    "transform": {
      "x": "canvas.width * 0.5",
      "y": "canvas.height * 0.5",
      "width": "canvas.width",
      "height": "canvas.height",
      "rotation": 0.0,
      "anchor_x": 0.5,
      "anchor_y": 0.5,
      "scale_x": 1.0,
      "scale_y": 1.0,
      "skew_x": 0.0,
      "skew_y": 0.0
    },
    "alignment": [0, 0],
    "fit": "cover"
  }
}
```

---

## Expression System

### Syntax

```
expr       = term (('+' | '-') term)*
term       = factor (('*' | '/') factor)*
factor     = unary | call | atom
unary      = ('-' | '+') factor
call       = IDENT '(' expr (',' expr)* ')'
atom       = NUMBER | '(' expr ')' | ref
ref        = IDENT '.' IDENT
```

### Built-in references

| Target | Properties |
|---|---|
| `canvas` | `width`, `height` |
| `timeline` | `frame`, `duration`, `fps` |
| `<clip_id>` | `x`, `y`, `width`, `height`, `opacity`, `rotation` |
| `<layout_node_id>` | `x`, `y`, `width`, `height` |

Clip property references reflect the **fully resolved** value for the current frame (after all
expressions in the evaluation order have run). The dependency graph guarantees expressions are
always evaluated after their dependencies.

### Error policy

- **Parse errors** → `CompileError` at compile time. Never silently fall back to zero.
- **Runtime eval errors** (division by zero, unresolved reference) → `RenderError` with the
  expression string and clip id for fast debugging.

---

## Compilation Pipeline

```
Project (JSON)
    │
    ▼
compile_project()
    │
    ├─ Validate: source IDs unique, referenced source IDs exist
    ├─ Validate: clip IDs unique across entire project
    ├─ Validate: mask items do not share IDs with layer-stack items
    ├─ Parse all StyleValue::Expr fields → ParsedExpr
    ├─ Build expression dependency graph (nodes = clip properties, edges = references)
    ├─ Detect cycles → CompileError::CircularDependency
    ├─ Topological sort → per-frame property evaluation order
    ├─ Flatten layer items → Vec<CompiledOperation> ordered by z-index
    ├─ Build per-frame operation index (frame → [operation indices])
    └─ Resolve purely-literal transforms into ClipPropertyIndex
            │
            ▼
       Arc<CompiledTimeline>   (Send + Sync)
```

`CompiledTimeline` is `Arc`-wrapped so it can be shared across the render thread pool without
copying. It is immutable after construction.

### Scaling

`compile_project_with_scale(project, scale)` multiplies all spatial literals by `scale`.
Expressions referencing `canvas.width` / `canvas.height` resolve correctly because the canvas
dimensions are also scaled before compilation.

### Module split

```
compile/
  mod.rs          # compile_project orchestration + CompileError
  validate.rs     # project and item validation
  sources.rs      # source indexing and source table assembly
  style.rs        # scalar registry + base/clip style compilation
  layout.rs       # layout-node compilation
  frame_index.rs  # frame -> operation index construction
  dependency.rs   # dependency graph construction, cycle detection, topological sort
  operation.rs    # CompiledOperation, CompiledTimeline, runtime frame context
  scalar.rs       # StyleValue parsing and scalar compilation
```

---

## Rendering Architecture

### Renderer trait

```rust
pub trait Renderer: Send {
    /// Render one frame. Returns RGBA row-major pixels (width × height × 4 bytes).
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError>;
}
```

The renderer is stateless with respect to the project. It owns only rendering resources
(Skia surface, font cache, layout cache).

### Skia backend

Software (CPU) raster rendering only. GPU backends are deferred — they add significant complexity
for marginal gains over multithreaded CPU rendering. The Skia backend module structure:

```
backend/skia/
  mod.rs        # SkiaRenderer, render_frame dispatch
  primitives.rs # draw_shape, draw_text, draw_image, draw_solid
  layout.rs     # LayoutRenderTree, taffy integration
  mask.rs       # simple and complex mask paths
  shadow.rs     # drop shadow via Skia image filters
```

### Frame provider

```rust
pub enum ProvidedFrame {
    Ready(FrameImage),
    Missing,
    EndOfStream,
}

pub trait FrameProvider: Send {
    fn image(&mut self, source_id: &str) -> Result<ProvidedFrame, ProviderError>;
    fn video_frame(
        &mut self,
        source_id: &str,
        source_frame: u64,
    ) -> Result<ProvidedFrame, ProviderError>;
}

pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}
```


`Missing` means the requested frame is not currently available but rendering may continue.
`EndOfStream` means the provider reached a terminal boundary for that source and frame mapping.
The renderer must treat these states explicitly and must not infer media state from `None`.
The renderer never caches media. Each `FrameProvider` implementation handles its own caching
(LRU, prefetch, etc.) independently of the renderer.

### Masking paths

1. **Simple** — rect/ellipse geometry on both sides → Skia clip operations, zero allocation.
2. **Complex** — anything else → `save_layer`, render content, render mask to offscreen surface,
   apply with `DstIn` blend, restore.

---

## Multithreaded Rendering

Expression-driven style properties are evaluated per-frame, which makes per-frame work more
expensive than a static property lookup. Parallelism is essential to compensate.

### Design

Different threads render different frames. The `CompiledTimeline` is read-only and shared via
`Arc`. Each worker thread owns:

- One `SkiaRenderer` (Skia surfaces are not `Sync`)
- One `FrameProvider` instance (video decoders are not shared across threads)

### Orchestrator

The `RenderOrchestrator` coordinates the thread pool:

```rust
pub struct RenderOrchestrator {
    thread_count: usize,
}

impl RenderOrchestrator {
    /// Render all frames in [start, end) and pass them in order to `on_frame`.
    /// `make_provider` is called once per worker thread to construct its FrameProvider.
    pub fn render_range<P, F>(
        &self,
        timeline: Arc<CompiledTimeline>,
        frame_range: Range<u64>,
        make_provider: impl Fn() -> P + Send + Sync,
        on_frame: F,
    ) -> Result<(), RenderError>
    where
        P: FrameProvider + 'static,
        F: FnMut(u64, Vec<u8>) -> Result<(), RenderError> + Send;
```


If `on_frame` returns `Err`, the orchestrator stops all workers immediately and propagates that
error to the caller. Frame sinks are part of the render contract and are never best-effort.
`make_provider` is a factory called on each worker thread, not on the calling thread — this allows
FFmpeg decoder contexts (which must be initialized on the thread that uses them) to be created
correctly.

### Frame ordering and output

Frames are rendered out of order across threads. The orchestrator uses a fixed-size reorder buffer
(size = `thread_count * 2`) to reassemble frames in sequence before delivering them to `on_frame`.
This bounds memory: at most `thread_count * 2` rendered frames live in memory simultaneously.

```
Worker 0: frame 0, 4, 8, ...
Worker 1: frame 1, 5, 9, ...
Worker 2: frame 2, 6, 10, ...
Worker 3: frame 3, 7, 11, ...
                  │
           reorder buffer (8 slots)
                  │
           on_frame(0, ...), on_frame(1, ...), ...  ← in order
```

If a worker finishes a frame but the reorder buffer is full (blocked on a slow earlier frame), that
worker waits. This applies natural back-pressure without unbounded buffering.

### Worker lifecycle

```
spawn N threads
each thread:
  provider = make_provider()
  renderer = SkiaRenderer::new(width, height)
  loop:
    frame = work_queue.pop()   // atomic fetch-and-increment of next frame index
    if frame >= end: break
    pixels = renderer.render_frame(&timeline, frame, &mut provider)
    result_tx.send((frame, pixels))
```

The work queue is a single `AtomicU64` counter shared across all workers. No thread has a
pre-assigned frame range — idle workers pull the next available frame. This self-balances
automatically when some frames are cheaper than others.

### Expression evaluation under parallelism

Expression evaluation during rendering is pure (reads `CompiledTimeline`, writes nothing). Each
call to `eval_scalar(compiled_expr, frame, ctx)` is a stack-local computation with no shared
mutable state. No locking is needed for expression evaluation.

---

## Video Frame Handling

Video frames are never fully loaded into memory. The flow per worker thread:

1. During `render_frame`, the renderer calls `provider.video_frame(source_id, source_frame)`.
2. The provider decodes only the requested frame (seeking if necessary) and returns it.
3. The provider maintains a per-thread LRU cache (default: 16 frames) to serve consecutive or
   repeated frame requests without re-decoding.

### Source frame mapping

`CompiledOperation::source_frame_at(timeline_frame) -> u64` maps a timeline frame to a source
frame by applying the video pipeline (trim, speed, loop). Speed may be negative (reverse). This
is deterministic and called by the renderer before requesting frames from the provider.

### Negative speed (reverse playback)

A `speed` of `-1.0` plays the source backwards. The frame mapping formula handles this:

```
effective_frame = if speed < 0 {
    trim.end - (elapsed_frames * |speed|) % source_length
} else {
    trim.start + (elapsed_frames * speed) % source_length
}
```

### Online sources

`kind.type = "url"` sources are downloaded and validated before compilation by the caller
(`lumen-server` / `lumen-local`). The `lumen` crate only references `file` paths. It never
performs network I/O.

---

## Rust Practices

These rules exist because previous agent iterations introduced specific recurring problems.

### Never use `include!()`

`include!()` pastes a file textually into the current scope. It defeats incremental compilation,
IDE navigation, and doc generation. Always use `mod`:

```rust
// Wrong
include!("primitives.rs");

// Right
mod primitives;
use primitives::draw_shape;
```

### Traits model real variation

Define a trait only when multiple implementations exist or a crate boundary requires abstraction.
Do not create single-implementation newtypes that add no behavior.

### Error types — `thiserror` in `lumen`, `anyhow` in binaries

- All errors in the `lumen` crate are `#[derive(thiserror::Error)]` enums with matchable variants.
- `lumen-server` and `lumen-local` use `anyhow::Result` internally.
- `unwrap()` and `expect()` are forbidden outside of test code. Use `?` or handle explicitly.
- Never silently discard errors (`let _ = ...`).

### Compile-time indices, not per-frame lookups

Clip IDs are `String` in the JSON model. After compilation, all internal references are indices:

```rust
// Good
pub struct CompiledOperation {
    source_index: usize,  // index into CompiledTimeline::sources
}

// Avoid
pub struct CompiledOperation {
    source_id: String,  // HashMap lookup every frame
}
```

### No logic duplication across crates

`lumen` owns compilation, expression evaluation, frame mapping, and rendering.
`lumen-server` owns decoding, encoding, networking, and job management.
If rendering logic appears in `lumen-server`, move it to `lumen`.

### File size — ~600 lines maximum

Split files that grow beyond this. The model and compilation modules use subdirectories:

```
model/
  mod.rs        # re-exports
  clip.rs       # ClipContent, ClipStyle, LayerItem
  source.rs     # Source, SourceKind, VideoPipeline
  layout.rs     # LayoutNode, LayoutNodeStyle
  project.rs    # Project, Canvas, Timeline, Layer

compile/
  mod.rs          # compile_project orchestration + CompileError
  validate.rs     # project and item validation
  sources.rs      # source indexing and source table assembly
  style.rs        # scalar registry + base/clip style compilation
  layout.rs       # layout-node compilation
  frame_index.rs  # frame -> operation index construction
  dependency.rs   # dependency graph, cycle detection, topological sort
  operation.rs    # CompiledOperation, CompiledTimeline
  scalar.rs       # StyleValue compilation

### Serde conventions

- `#[serde(rename_all = "snake_case")]` on every serializable type.
- `#[serde(default)]` only for fields with genuine defaults, not as a validation shortcut.
- `#[serde(deny_unknown_fields)]` where forward-compatibility must be explicit.
- `#[serde(skip_serializing_if = "Option::is_none")]` to keep JSON compact.

### `lumen` is synchronous

The renderer, compiler, and expression evaluator are all plain `fn`. No `async fn` in `lumen`.
Async lives in `lumen-server` and `lumen-wasm`. The `FrameProvider` trait is synchronous; any
async prefetch is an internal implementation detail of the provider.

### No GPU rendering (for now)

Software CPU raster only. Do not add Metal, Vulkan, or OpenGL backends until profiling demonstrates
a bottleneck that multithreaded CPU rendering cannot address.

### Owned types in public APIs

Public structs own their data. No lifetimes in public-facing types:

```rust
// Good
pub struct FrameImage { pub rgba: Arc<Vec<u8>> }

// Avoid
pub struct FrameImage<'a> { pub rgba: &'a [u8] }
```

### Tests

- Unit tests for pure logic (expression parsing, scalar evaluation, frame mapping, cycle detection)
  live in `#[cfg(test)] mod tests { }` in the same file.
- Integration tests that render a full frame live in `crates/lumen/tests/`.
- Every non-trivial function has at least one test.

### Do not over-abstract prematurely

Three similar concrete implementations are usually better than a premature abstraction. Introduce
generics and traits when the third real case confirms the pattern, not before.
