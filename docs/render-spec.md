# Render Semantics Spec

- Version: 0.1
- Date: 2026-02-13
- Scope: Canonical server render semantics and preview target semantics.

## 1) Timeline and Clip Ordering

1. Clips are active for `start_frame <= frame < end_frame`.
2. `end_frame = start_frame + duration_frames`.
3. Layers are sorted by ascending `z_index`.
4. For equal `z_index`, declaration order is stable and earlier clips render first.
5. Later-drawn clips visually compose over earlier clips.

## 2) Transform Semantics

Transform fields:

- `x`, `y` are top-left target placement in canvas pixel space.
- `width`, `height` define the target bounds for image/video fit; if unset, source dimensions are
  used.
- `rotation_degrees` rotates around the resolved draw rect center.
- `corner_radius` (image/video clips only) clips rendered media to rounded corners. `0` disables
  clipping.

Fit modes for image/video:

- `fill`: stretch to target bounds.
- `contain`: preserve aspect ratio; letterbox/pillarbox within target bounds.
- `cover`: preserve aspect ratio; crop overflow outside target bounds.

All transform numeric values must be finite.

## 3) Opacity and Alpha Composition

1. Clip opacity is clamped to `[0.0, 1.0]`.
2. Effective alpha = clip alpha * clip opacity.
3. Composition uses source-over layering in draw order.
4. Server canonical render assumes premultiplied alpha handling in the GPU backend.
5. Preview implementations must match source-over ordering and effective opacity behavior even when
   approximating effects.

## 4) Color and Pixel Assumptions

1. Canvas and clip colors are RGBA8.
2. Current pipeline assumes sRGB-like transfer for render and encode paths.
3. Backends must not silently reinterpret color channels or alpha ordering.
4. Output frame buffer contract for backend boundary: packed RGBA8, row-major, tightly packed.

## 5) Source Pipeline Semantics (Video)

Per-clip source mapping is deterministic and includes:

1. `trim` (optional): frame subrange.
2. `speed` (> 0 and finite): maps local frame progression.
3. `reverse` (requires bounded trim): reverse playback inside span.
4. `loop`:
   - none,
   - finite count (requires bounded trim),
   - infinite (requires bounded trim).

Out-of-range mapping returns no frame (`None`) and renders transparent for that operation.

## 6) Text Semantics (Current)

1. Font size is clamped to >= 1.0.
2. Multi-line text is split on newline boundaries.
3. Alignment applies per line (`left`, `center`, `right`) in target width.
4. Text color alpha participates in clip opacity multiplication.

## 7) Server vs Preview Contract

1. Server output is the source of truth for final renders.
2. Preview is best-effort and may approximate unsupported effects.
3. Unsupported preview behavior must follow explicit fallback policy in
   `docs/preview-parity-matrix.md`.
4. Preview must surface approximation state in UI for any non-parity effect.

## 8) Fixture and Benchmark Contract

1. Shared fixtures live in `docs/bench/fixtures/`.
2. Backend comparisons must use identical fixtures, resolutions, codecs, and machine class.
3. Any fixture change requires benchmark baseline refresh and report note.
