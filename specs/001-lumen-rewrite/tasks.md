# Tasks: Lumen Rendering Engine Rewrite

**Input**: Design documents from `specs/001-lumen-rewrite/`
**Prerequisites**: spec.md ✓, plan.md ✓
**Source**: Remaining items from `crates/lumen/TODO.md` organized by spec.md user stories

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story. All P0/foundational work (expressions, dependency sort, style resolution, API cleanup, clip geometry, transforms, fill/stroke, border radius, image/video pixel rendering) is already complete and is documented in the "Completed" section at the bottom of this file.

---

## Phase 1: Foundational Remaining (Blocks Multiple Stories)

**Purpose**: These items are prerequisites that don't belong to a single user story but unblock P1 text, layout, and shadow work.

- [X] T001 Add `FontCollection` cache field to `RendererContext` in `crates/lumen/src/render/context.rs` — construct once with `FontCollection::new()` + `set_default_font_manager(FontMgr::default(), None)`, store as `Arc<Mutex<FontCollection>>` or plain field

**Checkpoint**: `RendererContext` exposes a shared `FontCollection` — text clips can now build paragraphs without re-creating the collection per frame.

---

## Phase 2: User Story 7 — Text Rendering and Measurement (Priority: P1)

**Goal**: Replace the white-box placeholder in `TextClip::draw` with real Skia `textlayout::Paragraph` rendering, and expose `TextClip::measure()` for Taffy integration.

**Independent Test**: Render "Hello" in 24 px bold, assert non-transparent pixels appear within the clip bounds. Call `measure(200.0, ctx)` and assert returned height > 0.

### Implementation

- [X] T002 [US7] Implement `TextClip::draw` using `ParagraphBuilder` + `ParagraphStyle` + `SkTextStyle` in `crates/lumen/src/clip/text.rs` — resolve all `TextStyle` fields via `StyleContext`, call `paragraph.layout(max_width.unwrap_or(f32::MAX))`, call `paragraph.paint(canvas, (0.0, offset_y))`
- [X] T003 [US7] Apply `vertical_align` offset in `TextClip::draw` in `crates/lumen/src/clip/text.rs` — `Top`→0, `Middle`→`(height - paragraph.height()) / 2`, `Bottom`→`height - paragraph.height()`
- [X] T004 [US7] Handle `max_lines` + `TextOverflow::Ellipsis` in `ParagraphStyle` setup in `crates/lumen/src/clip/text.rs` — `para_style.set_max_lines(n)`, `para_style.set_ellipsis("…")`
- [X] T005 [US7] Add `TextClip::measure(available_width: f32, ctx: &StyleContext) -> (f32, f32)` in `crates/lumen/src/clip/text.rs` — builds paragraph, calls `layout(available_width)`, returns `(longest_line(), height())`
- [X] T006 [US7] Add font family fallback handling in `TextClip::draw` in `crates/lumen/src/clip/text.rs` — if resolved `font_family` is empty or unknown, rely on `FontMgr::default()` fallback already registered on `FontCollection`; document behavior in a code comment
- [X] T007 [P] [US7] Add text rendering tests in `crates/lumen/src/clip/text.rs` — test: non-transparent pixels in rendered output, `measure()` returns positive dimensions, wrapping produces height > single-line height, `max_lines: 1` + `Ellipsis` clips at one line

**Checkpoint**: `TextClip` renders real glyphs; `measure()` returns correct intrinsic dimensions.

---

## Phase 3: User Story 4 — Layout Clips with Text Measure Functions (Priority: P1 → P2)

**Goal**: Wire `TextClip::measure` into Taffy as a `MeasureFunc` for text content nodes; implement overflow clipping for layout containers.

**Independent Test**: Flex layout with a single text node, no fixed width. Assert Taffy computes a non-zero height matching `TextClip::measure`.

**Depends on**: Phase 2 complete (needs `TextClip::measure`).

### Implementation

- [X] T008 [US4] Register Taffy `MeasureFunc` for `LayoutContent::Text` nodes in `crates/lumen/src/clip/layout.rs` — after inserting each Taffy node, if `content == Some(LayoutContent::Text(clip))`, call `tree.set_measure_func(node_id, MeasureFunc::Boxed(...))` that delegates to `clip.measure(available_width, ctx)`
- [X] T009 [US4] Implement `overflow: Hidden` clipping in layout node render traversal in `crates/lumen/src/clip/layout.rs` — after translating canvas to computed position, if `node.style.overflow == Overflow::Hidden`, call `canvas.clip_rect(Rect::from_wh(w, h), ClipOp::Intersect, true)` before drawing content and children
- [X] T010 [P] [US4] Add layout integration tests in `crates/lumen/src/clip/layout.rs` — test: two `flex_grow: 1` shapes fill parent width equally; text node sized by intrinsic measure; `overflow: Hidden` clips out-of-bounds child; absolute-positioned child renders at inset offset

**Checkpoint**: Layout clips correctly size text via Taffy measurement; overflow clipping works.

---

## Phase 4: User Story 16 — Shadow Rendering with Proper Gaussian Blur (Priority: P1)

**Goal**: Replace the opacity-division blur approximation with true Gaussian blur via Skia `MaskFilter`; support multiple shadows, inset shadows, and `spread`.

**Independent Test**: Render a white rectangle with one shadow (`blur: 8`, offset 0/4, black). Assert pixels below the rectangle are darker than the background. Render with `inset: true`, assert dark pixels appear inside the rectangle's top edge.

### Implementation

- [X] T011 [US16] Change `BaseStyle::shadow: Option<ShadowStyle>` to `shadows: Vec<ShadowStyle>` in `crates/lumen/src/clip/style/base.rs` — update `BaseStyle::resolve`, `BaseStyle::draw`, and all call sites to use the `Vec`
- [X] T012 [US16] Add `spread: StyleProperty<f32>` and `inset: bool` fields to `ShadowStyle` in `crates/lumen/src/clip/style/base.rs`
- [X] T013 [US16] Implement proper outer shadow rendering in `BaseStyle::draw` shadow pass in `crates/lumen/src/clip/style/base.rs` — for each shadow with `inset == false`: save canvas, create `Paint` with shadow color, set `paint.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, blur / 2.0, false))`, expand rect by `spread`, draw shadow shape offset by `(offset_x, offset_y)`, restore canvas
- [X] T014 [US16] Implement inset shadow rendering in `BaseStyle::draw` in `crates/lumen/src/clip/style/base.rs` — for each shadow with `inset == true`: clip canvas to element shape, draw large-rect-minus-element shape (contracted by `spread`) with blur paint and offset, restore

**Checkpoint**: Shadows render with real Gaussian blur; multiple shadows stack in declaration order; inset shadows appear inside the element.

---

## Phase 5: User Story 8 — Masks (Priority: P2)

**Goal**: Add `Mask` to `BaseStyle` with three source types — shape (canvas clip path), bitmap (alpha composite), and clip (referenced clip's alpha channel).

**Independent Test**: Apply circular ellipse shape mask to a solid red rectangle. Render. Assert pixels inside the circle are red; outside are transparent.

**Depends on**: Phase 4 complete (base style draw is finalized).

### Implementation

- [X] T015 [US8] Define `MaskSource`, `MaskShape`, and `Mask` types in `crates/lumen/src/clip/style/base.rs` — `MaskSource`: `Shape(MaskShape)`, `Bitmap { source: String }`, `Clip { clip_id: String }`; `MaskShape`: `Rectangle { x, y, width, height, corner_radius: [StyleProperty<f32>; 4] }`, `Ellipse { cx, cy, rx, ry }`, `Path { data: Vec<PathCommand> }`; `Mask`: `source: MaskSource`, `inverted: bool`
- [X] T016 [US8] Add `mask: Option<Mask>` to `BaseStyle` struct in `crates/lumen/src/clip/style/base.rs` — add field, update `Default`, update `BaseStyle::resolve`
- [X] T017 [US8] Implement `MaskSource::Shape` rendering in `BaseStyle::draw` in `crates/lumen/src/clip/style/base.rs` — build Skia `Path` from resolved `MaskShape` geometry, call `canvas.clip_path(&path, if inverted { ClipOp::Difference } else { ClipOp::Intersect }, true)` before the draw callback
- [X] T018 [US8] Implement `MaskSource::Bitmap` rendering in `BaseStyle::draw` in `crates/lumen/src/clip/style/base.rs` — render clip content to temp surface, decode mask image to another surface, composite using `BlendMode::DstIn` (or `DstOut` if inverted), draw result onto main canvas
- [X] T019 [US8] Implement `MaskSource::Clip` rendering in `BaseStyle::draw` in `crates/lumen/src/clip/style/base.rs` — same as Bitmap but render the referenced clip to the mask surface instead of decoding an image; clip must have been rendered before this call (dependency edge ensures ordering)
- [X] T020 [US8] Add dependency edge for `MaskSource::Clip` in `crates/lumen/src/dependency/mod.rs` — when scanning a clip's `BaseStyle`, if `mask.source == MaskSource::Clip { clip_id }`, insert edge `DependencyNode::ClipRender(clip_id) → DependencyNode::ClipRender(current_clip_id)`

**Checkpoint**: All three mask types work; clip mask dependency ordering is enforced by the resolver.

---

## Phase 6: User Stories 13 + 14 — FFmpeg Decode Pipeline (Priority: P2, feature = "ffmpeg")

**Goal**: Implement `LibavStreamDecoder` with RGBA swscale conversion, LRU frame cache, buffer recycling, seek/reopen fallback, and hardware decode with graceful software fallback.

**Independent Test** (behind `#[cfg(feature = "ffmpeg")]`): Open a test video, request frame 0, then 100, then 50 (backward seek). Assert all three frames return non-empty RGBA data. Assert LRU cache contains frame 100 after the second call.

### Implementation

- [X] T021 [US13] Implement FFmpeg global one-time initialization via `OnceLock<Result<(), String>>` in `crates/lumen/src/ffmpeg/mod.rs` — `fn ensure_ffmpeg_init() -> Result<(), FfmpegError>` calls `ffmpeg::init()` exactly once, sets log level to Error
- [X] T022 [US13] Define `LibavStreamDecoder` struct in `crates/lumen/src/ffmpeg/mod.rs` — fields: `input_ctx`, `video_stream_index`, `decoder`, `scaler`, `width`, `height`, `time_base`, `timeline_time_base`, `next_source_frame`, `cache: LruCache<u64, FrameImage>`, `buffer_pool: Vec<Vec<u8>>`, `decoded_frame`, `scratch_frame`, `packet`, `eof`, `draining`, `last_decoded_source_frame`, `last_decoded_image`
- [X] T023 [US13] Implement `LibavStreamDecoder::new(path, fps: Rational, cache_frames: usize)` in `crates/lumen/src/ffmpeg/mod.rs` — open input context, find video stream, open decoder, create `scaling::Context` (native format → RGBA, FAST_BILINEAR), read `time_base` and compute `timeline_time_base` from `fps`
- [X] T024 [US13] Implement `source_frame_to_pts` and `pts_to_source_frame` methods on `LibavStreamDecoder` in `crates/lumen/src/ffmpeg/mod.rs` — PTS ↔ frame index conversion using `time_base` and `timeline_time_base` rationals
- [X] T025 [US13] Implement `get_frame(target: u64) -> Result<Option<FrameImage>, FfmpegError>` on `LibavStreamDecoder` in `crates/lumen/src/ffmpeg/mod.rs` — (1) LRU cache check; (2) if behind, seek or reopen; (3) decode-forward loop; (4) swscale conversion with stride handling; (5) store in LRU; (6) return nearest if exact not cached
- [X] T026 [US13] Implement swscale RGBA conversion with stride handling in `crates/lumen/src/ffmpeg/mod.rs` — after `scaler.run(&decoded_frame, &mut scratch_frame)`, copy `width * 4` bytes per row (not full stride) into a pooled `Vec<u8>`
- [X] T027 [US13] Implement gap filling in `get_frame` in `crates/lumen/src/ffmpeg/mod.rs` — when `decoded_source_frame - last_decoded_source_frame > 1`, insert the last `FrameImage` into the LRU for all gap indices
- [X] T028 [US13] Implement LRU eviction with buffer recycling in `crates/lumen/src/ffmpeg/mod.rs` — on LRU eviction, attempt `Arc::try_unwrap` on the pixel buffer; if successful and capacity matches `frame_byte_size`, push back into `buffer_pool`
- [X] T029 [US13] Implement seek/reopen fallback in `get_frame` in `crates/lumen/src/ffmpeg/mod.rs` — attempt `input_ctx.seek(target_pts, ..target_pts)`; on failure (non-seekable source or error), call a `reopen_and_skip(target_frame)` helper that re-opens the source and decodes forward from frame 0
- [X] T030 [US14] Implement hardware decode attempt in `LibavStreamDecoder::new` in `crates/lumen/src/ffmpeg/mod.rs` — read `LUMEN_LIBAV_HW_DEVICE` env var; if `"auto"`, try each platform-specific device type via `av_hwdevice_ctx_create`; if all fail or env var absent, proceed with software decode; log warning on HW failure
- [X] T031 [P] [US14] Add `unsafe impl Send for LibavStreamDecoder` in `crates/lumen/src/ffmpeg/mod.rs` with doc comment: "Safe: decoder is owned exclusively by its worker thread and accessed only via the bounded channel"
- [X] T032 [P] [US13] Add decoder tests behind `#[cfg(feature = "ffmpeg")]` in `crates/lumen/src/ffmpeg/mod.rs` — test: PTS round-trip for known frame numbers; sequential forward decode returns correct frame count; backward seek returns correct frame; LRU cache hit after sequential forward; gap filling populates intermediate indices

**Checkpoint**: `LibavStreamDecoder` decodes any FFmpeg-supported source to RGBA, caches frames in a bounded LRU, and handles backward seeks.

---

## Phase 7: User Story 15 — Video Decode Worker Threads with Prefetch (Priority: P2)

**Goal**: Wrap `LibavStreamDecoder` in a dedicated background thread per video source. Detect forward/reverse sequential access and prefetch frames into the LRU cache.

**Independent Test**: Spawn a worker, request frame 10, then frame 11. Inspect cache: frames 12 through 10+prefetch_count should already be cached before the frame 11 reply is sent (test via a dedicated test hook or by timing subsequent requests).

**Depends on**: Phase 6 complete.

### Implementation

- [X] T033 [US15] Define `DecodeRequest` and `VideoDecodeWorker` structs in `crates/lumen/src/ffmpeg/worker.rs` — `DecodeRequest { source_frame: u64, reply: SyncSender<Result<Option<FrameImage>, FfmpegError>> }`; `VideoDecodeWorker { source_id, tx: Option<SyncSender<DecodeRequest>>, handle: Option<JoinHandle<()>> }`
- [X] T034 [US15] Implement `VideoDecodeWorker::spawn` in `crates/lumen/src/ffmpeg/worker.rs` — read config env vars (`LUMEN_LIBAV_PREFETCH_QUEUE`, `LUMEN_LIBAV_PREFETCH_FRAMES`), create bounded `sync_channel`, spawn worker thread via `thread::spawn(move || run_decode_worker(...))`
- [X] T035 [US15] Implement `VideoDecodeWorker::get_frame` in `crates/lumen/src/ffmpeg/worker.rs` — create one-shot reply channel, send `DecodeRequest`, block on `reply_rx.recv()`
- [X] T036 [US15] Implement `Drop for VideoDecodeWorker` in `crates/lumen/src/ffmpeg/worker.rs` — `self.tx.take()` closes the sender → worker loop exits; `self.handle.take().join()` waits for clean shutdown
- [X] T037 [US15] Implement `run_decode_worker` in `crates/lumen/src/ffmpeg/worker.rs` — recv loop: detect direction (Forward/Reverse/Random) by comparing current and last frame numbers; send reply; then prefetch next N frames in the detected direction
- [X] T038 [US15] Implement `StreamingAssets` + `FrameProvider` impl in `crates/lumen/src/render/backend/mod.rs` — `StreamingAssets { images: HashMap<String, FrameImage>, video_workers: HashMap<String, VideoDecodeWorker> }`; implement `FrameProvider::image` and `FrameProvider::video_frame` returning `ProvidedFrame` variants
- [X] T039 [US15] Add `ProvidedFrame` enum to `crates/lumen/src/render/backend/mod.rs` — `Ready(FrameImage)`, `EndOfStream`, `Missing`
- [X] T040 [US15] Implement optional encoder thread path (`render_to_mp4`) in `crates/lumen/src/ffmpeg/worker.rs` — bounded `sync_channel::<Vec<u8>>(LUMEN_ENCODE_QUEUE)`, spawn `encode_rgba_stream` thread, render loop sends RGBA frames, drop sender signals EOF, join encode thread
- [X] T041 [P] [US15] Add thread-safety documentation in `crates/lumen/src/ffmpeg/worker.rs` and `crates/lumen/src/ffmpeg/mod.rs` — doc comments covering: `LibavStreamDecoder` `unsafe Send` invariant, `FrameImage` as `Arc<Vec<u8>>` for cheap cross-thread clone, Skia surfaces must stay on render thread, `VideoDecodeWorker` channel as the only access path

**Checkpoint**: Each video source decodes on its own thread; sequential playback is smooth via prefetch; workers shut down cleanly on drop.

---

## Phase 8: User Stories 17 + Scene/Layer Model and Render Pipeline (Priority: P2)

**Goal**: Add `Scene` and `Layer` structs; build a structured render pipeline that resolves dependencies, computes layouts, draws clips, and composites layers with blend modes and animated opacity.

**Independent Test**: Two layers — red background, blue `BlendMode::Normal` overlay at 50 % opacity. Assert output pixels in overlap are between pure red and pure blue (blended).

**Depends on**: Phases 2–7 preferred but can be done in parallel once Phase 2 (text) is done.

### Implementation

- [X] T042 [US17] Create `crates/lumen/src/scene.rs` with `Scene` and `Layer` structs — `Scene { width: u32, height: u32, frame_rate: Rational, duration_frames: u32, layers: Vec<Layer> }`; `Layer { id: String, clips: Vec<ClipType>, blend_mode: BlendMode, opacity: StyleProperty<f32>, visible: bool }`
- [X] T043 [US17] Add `pub mod scene;` to `crates/lumen/src/lib.rs` and re-export `Scene`, `Layer`
- [X] T044 [US17] Implement structured render pipeline in `crates/lumen/src/render/mod.rs` — stages: validate frame range → collect expressions → `DependencyPlan::build` → topo sort → iterate `evaluation_order` with `ResultMap` → layer compositing loop → `read_surface_rgba`
- [X] T045 [US17] Implement layer compositing in `crates/lumen/src/render/mod.rs` — for each `Layer` where `visible == true`: create a layer-sized Skia surface, draw all clips in the layer to it, composite the layer surface onto the main canvas using the layer's resolved `blend_mode` and `opacity`
- [X] T046 [US17] Guard `frame >= scene.duration_frames` in the render pipeline entry point in `crates/lumen/src/render/mod.rs` — return `RenderError::OutOfRange { frame, duration: scene.duration_frames }`
- [X] T047 [P] [US17] Add layer compositing tests in `crates/lumen/src/render/mod.rs` — test: `visible: false` layer contributes no pixels; `BlendMode::Normal` layer at 50 % opacity blends correctly; layer opacity keyframes animate correctly across frames; out-of-range frame returns error

**Checkpoint**: `Scene` is the single entry point for rendering; layers composite correctly with blend modes and animated opacity.

---

## Phase 9: Polish and Cross-Cutting Concerns

**Purpose**: Observability, ergonomics, and test coverage that span all user stories.

- [X] T048 [P] Add `tracing` span and event hooks at render pipeline stage boundaries in `crates/lumen/src/render/mod.rs` — one span per stage (dependency resolve, layout compute, clip draw, layer composite, pixel readback); log frame number and timing in each span
- [X] T049 [P] Add builder / constructor helpers for common clip + style setup in `crates/lumen/src/clip/` — e.g., `ShapeClip::rect(x, y, w, h, fill)`, `TextClip::simple(x, y, content, font_size)`, `ImageClip::new(x, y, w, h, source, fit)` — convenience constructors that pre-fill common fields
- [X] T050 [P] Add deterministic software-render snapshot tests in `crates/lumen/src/render/backend/software.rs` — render known scenes (solid red rect, text clip, two-layer blend) and compare RGBA output byte-by-byte against committed reference PNG fixtures; fail on any pixel diff
- [X] T051 [P] Add property-based fuzz tests for expression parsing and dependency resolution in `crates/lumen/src/expr/mod.rs` and `crates/lumen/src/dependency/tree.rs` — use `proptest` or `quickcheck`; assert: any random expression string either parses successfully or returns `ExpressionError::ParseError` (never panics); any DAG topology produces a valid topological order; any graph with a cycle returns `Cycle` error

---

## Dependencies and Execution Order

### Phase Dependencies

- **Phase 1** (FontCollection cache): No dependencies — can start immediately.
- **Phase 2** (Text rendering): Depends on Phase 1.
- **Phase 3** (Layout measure func): Depends on Phase 2 (`TextClip::measure`).
- **Phase 4** (Shadow Gaussian blur): No dependencies on Phases 2–3 — can run in parallel with Phase 2.
- **Phase 5** (Masks): Depends on Phase 4 (base style draw finalized).
- **Phase 6** (FFmpeg decoder): No dependencies on Phases 2–5 — can run fully in parallel.
- **Phase 7** (Worker threads): Depends on Phase 6.
- **Phase 8** (Scene/Layer model): Depends on Phase 2 (text needed for meaningful scene tests); Phase 7 is a soft dependency (scene works without FFmpeg if sources are CPU-decoded).
- **Phase 9** (Polish): Depends on all above phases being substantially complete.

### User Story Dependencies

| User Story | Depends On | Can Parallelize With |
|------------|------------|---------------------|
| US7 Text (Ph 2) | Phase 1 | US16 Shadow (Ph 4), US13/14 FFmpeg (Ph 6) |
| US4 Layout (Ph 3) | US7 Text | US16 Shadow, US13/14 FFmpeg |
| US16 Shadow (Ph 4) | None | US7 Text, US13/14 FFmpeg |
| US8 Masks (Ph 5) | US16 Shadow | US13/14 FFmpeg, US15 Workers |
| US13/14 FFmpeg (Ph 6) | None | US7, US16, US8 |
| US15 Workers (Ph 7) | US13/14 FFmpeg | US8 Masks |
| US17 Scene (Ph 8) | US7 Text (soft) | US8, US15 |
| Polish (Ph 9) | All above | — |

### Within Each Phase

- Implementation tasks before their corresponding tests.
- Type/struct definitions before methods that use them.
- Core functionality before edge case handling (e.g., T022 struct before T025 `get_frame`).

### Parallel Opportunities

**Phase 2 (Text)**:
```
T002 (draw) + T003 (vertical align) → T004 (max_lines) → T005 (measure) → T006 (fallback)
T007 (tests) — can start after T005
```

**Phase 6 (FFmpeg) — high parallelism after T022 struct is defined**:
```
T021 (init) + T022 (struct) → [T023, T024 in parallel] → [T025, T026, T027, T028, T029 in parallel] → T030 (HW) → T031 (Send) → T032 (tests)
```

**Phase 9 — all tasks are parallel**:
```
T048 (tracing) + T049 (builders) + T050 (snapshots) + T051 (fuzz) — all fully independent
```

---

## Implementation Strategy

### MVP First (User Story 7 — Text Rendering)

1. Phase 1: Add `FontCollection` cache (T001)
2. Phase 2: Implement `TextClip::draw` + `measure()` (T002–T007)
3. **Validate**: Run `cargo test` — text renders, measure returns positive dimensions
4. Phase 3: Wire text measure into layout (T008–T010)
5. **Validate**: Layout clip sizes text nodes correctly

### Parallel Track (can run alongside MVP)

- Phase 4 (Shadow blur): T011–T014 — independent of text
- Phase 6 (FFmpeg): T021–T032 — entirely independent, behind feature gate

### Incremental Delivery

1. Phase 1 + 2 → Text renders ✓
2. Phase 3 → Layout sizes text via Taffy ✓
3. Phase 4 → Shadows use real Gaussian blur ✓
4. Phase 5 → Masks work (all three types) ✓
5. Phase 6 → FFmpeg decodes video to RGBA ✓
6. Phase 7 → Video decodes on background threads with prefetch ✓
7. Phase 8 → Scene/Layer model with blend compositing ✓
8. Phase 9 → Observability, builder helpers, snapshot tests, fuzz ✓

---

## Notes

- `[P]` tasks = different files, no dependencies — safe to parallelize
- `[US#]` label maps each task to its user story from `spec.md` for traceability
- All tasks behind the `ffmpeg` feature (Phases 6–7) must be compiled with `cargo test --features ffmpeg`
- Snapshot reference images (Phase 9, T050) should be committed to the repository alongside the test source
- `cargo fmt` and `cargo clippy --deny warnings` should pass after every phase

---

## Completed (Reference)

The following work from `crates/lumen/TODO.md` is **done** and does not need to be re-implemented:

| Category | What Was Completed |
|----------|--------------------|
| Architecture | `RendererContext`/`FrameContext` split; `render/backend/` tree; Software/Metal/Vulkan backends; `RenderBackend` trait; `read_surface_rgba`; `pixel_len` |
| Clip model | `ClipMeta`; `Clip` trait returning `Result`; `ClipType` dispatch; draw entry points for all clip variants |
| Style model | `BaseStyle::resolve` + `BaseStyle::draw` (method-based); `resolve_style_value` → `StyleProperty::resolve`; frame-aware `Sequence` resolution; `Interpolate` trait (f32/u8/u32/bool); all easing variants; expression graceful fallback |
| API cleanup | `ShapeKind::draw` methods; all clip modules use method-based APIs; `resolve_base_style` → method; `draw_with_base_style` → method |
| Clip geometry | Explicit `x/y/width/height/anchor` on all clips; transforms: `translate[2]`, `scale[2]`, `rotation`, `skew[2]`, `origin[2]`; CSS-compatible application order |
| Shape fidelity | Fill model (Solid, LinearGradient, RadialGradient, Image); Stroke model (color/width/cap/join/dash); rectangle corner radius `[4]`; `BaseStyle::clip_radius [4]` |
| Text style | `TextStyle` expanded with all FR-020 fields (font_family, font_size, font_weight, font_style, color[4], line_height, letter_spacing, text_align, vertical_align, max_width, max_lines, overflow, decoration) |
| Layout | `LayoutContent` enum; `LayoutNodeContext` stores content; Taffy-computed bounds drawn (not debug outlines) |
| Media | `ImageClip` renders decoded pixels; image fit modes (cover/contain/fill/none); Skia image caching by source_id; `VideoClip` renders decoded frames; `VideoClip::map_to_source_frame` (trim/speed/loop) |
| Expressions | `Expression::parse` / `Expression::evaluate`; full AST (BinaryOp, UnaryOp, FuncCall, Conditional, ClipRef, LayoutRef); all built-in math functions; all error variants |
| Dependency | `DependencyPlan::build`; `DependencyTree::topological_order` (Kahn); cycle detection; `evaluation_order` populated |
| Errors | `RenderError` enriched with clip id + frame context |
| Tests | Expression unit tests; style resolution tests (literal, keyframe, interpolation, easing); backend contract tests; media resolver tests; transform resolution tests; base style regression tests |
