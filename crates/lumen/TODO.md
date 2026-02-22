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

### Core render loop integration

- [ ] Implement a scene/timeline renderer that owns clip ordering and traverses clips per frame.
- [ ] Decide and implement deterministic z-order/layer ordering semantics.
- [ ] Define and implement mask/clip-path semantics across groups and clips.

### Backend depth
- [ ] Implement actual Metal GPU backend execution path (context/device/surface lifecycle).
- [ ] Implement actual Vulkan GPU backend execution path (instance/device/queue/surface lifecycle).
- [ ] Add backend capability detection and backend selection strategy.
- [ ] Add backend-specific error taxonomy and fallback policy.

### Styling correctness

- [ ] Resolve `StyleProperty::Sequence` by frame/time with interpolation.
- [ ] Resolve `StyleValue::Expression` against runtime expression context.
- [ ] Define precedence rules when literal, sequence, and expression sources coexist.
- [ ] Replace blur/shadow approximations with proper Skia image filter usage.
- [ ] Expand transform model beyond scalar translate/scale as needed (rotation, per-axis transforms).

### Text rendering

- [ ] Add actual text shaping and glyph rendering path.
- [ ] Support font selection, font loading, and fallback fonts.
- [ ] Support text layout details (line wrapping, alignment, overflow modes).

### Media rendering

- [ ] Decode image bytes and upload/blit into Skia image surfaces.
- [ ] Decode video frames and draw actual pixel content (not placeholder geometry).
- [ ] Add fit modes, crop, corner radius, and alpha compositing for media clips.
- [ ] Add media frame caching and invalidation policy.

### Layout rendering

- [ ] Render actual Taffy layout tree outputs per node bounds/styles.
- [ ] Bridge layout node identifiers to drawable clip/content nodes.
- [ ] Define overflow and clipping behavior for layout nodes.

### Expressions and dependencies

- [ ] Implement parser and evaluator for expression sources.
- [ ] Implement property resolution across clips and layout nodes.
- [ ] Build dependency graph from expression references.
- [ ] Implement topological evaluation order with cycle diagnostics.
- [ ] Add frame-time aware expression caching/invalidation.

### Testing and reliability

- [ ] Add unit tests for style resolution (literal/sequence/expression).
- [ ] Add tests for missing source and resolver error paths.
- [ ] Add backend contract tests (readback dimensions, alpha format, clear behavior).
- [ ] Add snapshot tests for deterministic rendering output.
- [ ] Add fuzz/property tests for expression/dependency edge cases.

### API/ergonomics

- [ ] Add builder or constructor helpers for clips/styles/contexts.
- [ ] Improve error messages with clip id and frame context.
- [ ] Add tracing/log hooks for frame render phases and timing.
