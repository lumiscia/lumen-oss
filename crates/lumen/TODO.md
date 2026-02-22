# TODO

This file tracks renderer work that is complete and remaining.

## Completed

### Architecture and module layout

- [x] Split rendering surface state into `RendererContext` and per-frame state into `FrameContext`.
- [x] Added backend module tree under `src/render/backend/`.
- [x] Added software, metal, and vulkan backend modules.
- [x] Unified backend contract under `RenderBackend` trait.
- [x] Added pixel readback helper (`read_surface_rgba`) and size overflow guard (`pixel_len`).

### Clip model and draw wiring

- [x] Added `ClipMeta` for shared clip identity and timing.
- [x] Updated `Clip` trait to use shared meta and return `Result<(), RenderError>` from draw.
- [x] Implemented `ClipType` dispatch for all clip variants.
- [x] Implemented draw entry points for group, layout, shape, text, image, and video clips.

### Style model and base style application

- [x] Added base style resolver (`resolve_base_style`) and resolved structs.
- [x] Added shared draw wrapper (`draw_with_base_style`) for style application.
- [x] Wired style wrapper into group/layout/shape/text/media draws.
- [x] Added shape support for rectangle, ellipse, and polygon variants.
- [x] Added width/height style properties for ellipse and rect shape styles.

### Media integration

- [x] Made `MediaStore` object-safe and injectable into `RendererContext`.
- [x] Added renderer-context media store setter/getter.
- [x] Wired image/video clips to resolver access through renderer context.
- [x] Enforced missing video resolver/media store as hard render error.

### Documentation

- [x] Added detailed implementation report in `README.md`.

## Remaining

### P0: Stubbed systems (highest leverage)

- [x] Replace `DependencyTree::topological_order` stub with Kahn topological sort.
- [x] Return cycle errors only when a real cycle exists (include deterministic behavior).
- [x] Replace `build_dependency_plan` free function with `DependencyPlan::build`.
- [x] Populate `DependencyPlan::evaluation_order` from the dependency tree.
- [x] Replace `parse_expression` / `evaluate_expression` free functions with `Expression::parse` / `Expression::evaluate`.
- [x] Parse expression references (`clip('id').property`, `layout('id').property`) and collect spans.
- [x] Implement expression AST + evaluator for literals, unary/binary ops, comparisons, logical ops.
- [x] Implement built-in math helpers (`min`, `max`, `abs`, `floor`, `ceil`, `round`, `clamp`, `lerp`, `sin`, `cos`).
- [x] Add expression error variants for parse, unknown function, type mismatch, and unresolved reference.
- [x] Add unit tests covering expression parsing/evaluation success paths and failures.

### P0: Style resolution correctness

- [x] Move `resolve_style_value` / `resolve_style_value_or` onto `StyleProperty<T>` as methods.
- [x] Make keyframe APIs constructible outside the module (`Keyframe`, `Sequence` helpers or public fields).
- [x] Add frame-aware resolution for sequences (`resolve(frame)` / `resolve_or(frame, fallback)`).
- [x] Add interpolation trait(s) for animatable property types used today (`f32`, `u8`, `u32`, `bool`).
- [x] Add easing support on keyframes (at least linear + common easings).
- [x] Keep expression values safe during resolution (graceful fallback when unresolved).
- [x] Add unit tests for literal resolution, before/after keyframes, exact keyframes, interpolation, and easing.

### P0: Style/base API cleanup (report style conventions)

- [x] Move `resolve_base_style` free function onto `BaseStyle::resolve`.
- [x] Move `draw_with_base_style` free function onto `BaseStyle::draw`.
- [x] Convert shape drawing helpers into methods (`ShapeKind::draw` and/or per-style `draw` methods).
- [x] Update all clip modules to use method-based style APIs.
- [x] Add regression tests for base style resolution clamps/defaults.

### P1: Clip geometry and transforms

- [ ] Add explicit clip geometry (x/y/width/height/anchor) instead of frame-relative debug placement.
- [ ] Thread resolved geometry through clip draw implementations.
- [ ] Replace scalar translate/scale with per-axis transform model plus rotation/skew/origin.
- [ ] Add transform resolution + application ordering tests.

### P1: Shape rendering fidelity

- [ ] Add fill model (solid first, gradients/images later).
- [ ] Add stroke model (width/color/cap/join/dash).
- [ ] Add rectangle corner radius (per-corner animatable).
- [ ] Add clip radius support in `BaseStyle` for generic round clipping.
- [ ] Replace shadow blur approximation with proper Skia blur mask filters.
- [ ] Support multiple shadows and inset shadows.

### P1: Text rendering and measurement

- [ ] Expand `TextStyle` with font, size, weight, color, spacing, alignment, wrapping fields.
- [ ] Implement real text shaping/rendering via Skia `textlayout::Paragraph`.
- [ ] Cache font collection / paragraph setup where appropriate.
- [ ] Expose text measurement for layout integration.
- [ ] Add text rendering and wrapping tests (metrics and/or pixel snapshots).

### P1: Layout clip rendering

- [ ] Add `LayoutContent` to layout nodes so nodes can render clips.
- [ ] Store content in `LayoutNodeContext`.
- [ ] Render Taffy-computed node bounds instead of debug outlines only.
- [ ] Add text measure functions for layout text nodes.
- [ ] Define overflow/clipping semantics for layout nodes.
- [ ] Add layout integration tests (nested positions/sizes).

### P1: Media rendering

- [x] Render decoded image pixels to Skia (`ImageClip`) instead of placeholders.
- [ ] Add image fit modes (`cover`, `contain`, `fill`, `none`).
- [ ] Add image/video Skia image caching to avoid per-frame conversions.
- [ ] Render decoded video frames to Skia (`VideoClip`) instead of placeholders.
- [x] Implement `VideoClip` timeline mapping for trim/speed/loop.
- [ ] Add media resolver tests for missing sources, frame mapping, and fallback behavior.

### P2: FFmpeg decode pipeline (feature-gated)

- [ ] Implement FFmpeg global one-time initialization.
- [ ] Add feature-gated libav stream decoder with RGBA conversion.
- [ ] Add bounded LRU frame cache and buffer recycling.
- [ ] Implement seek/reopen fallback for reverse/random access.
- [ ] Port optional hardware decode setup with graceful fallback.
- [ ] Add decoder tests around PTS/frame mapping and cache behavior (behind feature gate).

### P2: Threading and streaming asset pipeline

- [ ] Add per-source video decode worker threads with bounded request queues.
- [ ] Add forward/reverse prefetch behavior.
- [ ] Add `FrameProvider` integration for streaming assets.
- [ ] Add optional encoder thread path for MP4 pipelines (backpressure via sync channel).
- [ ] Document thread-safety invariants (`LibavStreamDecoder`, `FrameImage`, Skia surfaces).

### P2: Scene/layer model and render pipeline

- [ ] Introduce explicit `Scene` / `Layer` model (z-order, blend, opacity, visibility).
- [ ] Add structured render stages (dependencies -> layout -> draw -> composite -> readback).
- [ ] Define mask semantics (shape, bitmap, clip masks) and implement base-style mask support.
- [ ] Add layer compositing tests.

### P2: Reliability, ergonomics, and auditability

- [ ] Improve render errors with clip id/frame context where available.
- [ ] Add tracing/log hooks for render phases and timings.
- [ ] Add builder/constructor helpers for common clip/style setup.
- [ ] Add backend contract tests (readback dimensions, alpha format, clear behavior).
- [ ] Add deterministic software-render snapshot tests.
- [ ] Add fuzz/property tests for expression/dependency edge cases.
