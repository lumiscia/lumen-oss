# Render Semantics Spec

- Version: 0.1
- Date: 2026-02-21
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

## 2.1) Clip Animation Semantics

Each clip may optionally animate scalar properties with `animation` tracks:

- `opacity`, `x`, `y`, `width`, `height`, `rotation_degrees`
- Each track is an ordered list of keyframes: `frame`, `value`, `duration_frames`, `easing`

Semantics:

1. Animation frames are local to the clip (`0` is clip start).
2. A keyframe means "transition from the current property value to `value` starting at `frame`".
3. Transition duration is `duration_frames`; `0` means an immediate step at that frame.
4. Easing curves:
   - `linear`
   - `ease_in`
   - `ease_out`
   - `ease_in_out`
5. Tracks must not overlap in time for the same property.
6. `width`/`height` tracks require base `transform.width`/`transform.height` to be set and all
   animated values > 0.
7. Resolved opacity is clamped to `[0.0, 1.0]` before compositing.

## 2.2) Scalar Expression Semantics

Scalar fields may be either a numeric literal or an expression string.

Expression-capable fields:

- Clip transform: `x`, `y`, `width`, `height`
- Group transform: `x`, `y`
- Clip animation keyframe `value` for scalar tracks
- Layout node style dimensions: `width`, `height`, `min_width`, `min_height`, `max_width`,
  `max_height`

### Grammar and Operators

Expressions support full arithmetic composition (not just single binary ops), including unary
operators and parentheses.

```text
expr        := add_sub
add_sub     := mul_div (('+' | '-') mul_div)*
mul_div     := unary (('*' | '/') unary)*
unary       := ('+' | '-') unary | primary
primary     := number | reference | '(' expr ')'
reference   := ident '.' property
ident       := [A-Za-z_][A-Za-z0-9_]*
property    := 'width' | 'height' | 'x' | 'y'
```

Notes:

- Numbers support decimal and scientific notation (for example `12`, `12.5`, `1e3`).
- Whitespace is allowed.
- Unknown properties are invalid.

### Reference Namespaces

- `canvas.width`, `canvas.height` are valid.
- `canvas.x`, `canvas.y` are invalid.
- `<clip_or_group_id>.x|y|width|height` is valid when that id exists and the property exists.
- `<layout_node_id>.x|y|width|height` is valid only in runtime-evaluated layout-aware contexts.

### Resolution Timing

1. Compile-time transform index:
   - All clip/group transforms are resolved iteratively against known ids plus `canvas`.
   - Cycles or unresolvable dependency chains fail compilation.
2. Compile-time scalar resolution:
   - Expressions referencing only `canvas`/clip/group values resolve immediately.
3. Deferred runtime expression resolution:
   - Layout node dimension expressions that reference layout node ids are preserved for runtime.
   - Keyframe expressions that include layout-node references are preserved for runtime.
   - If a project contains layout nodes, unresolved id-like keyframe refs are treated as deferred
     layout-time refs instead of hard-failing at compile-time.

### Runtime Expression Context

When evaluating deferred expressions, runtime context includes:

- Static clip index (`canvas` + compiled clip/group transforms)
- Per-frame measured layout node values (`width`, `height`, `x`, `y`)

Deferred layout dimension evaluation is done after an initial layout pass, then layout is recomputed
if any deferred dimensions changed.

### Keyframe Expression Behavior

1. Keyframes are still applied in frame order with no-overlap rules.
2. A keyframe target expression is evaluated at runtime when reached.
3. If deferred expression evaluation fails for that frame (for example unresolved reference), that
   keyframe is skipped for that frame and animation keeps the current value.
4. Interpolation/easing behavior is unchanged once a keyframe target is resolved.

### Scaling Rules

- Numeric literals in scalar fields are multiplied by compile scale.
- Compile-time-resolved expression results are multiplied once by compile scale.
- Deferred expression literals are scaled before runtime evaluation so runtime values remain in
  scaled pixel space.

### Validation and Errors

- Empty/malformed expressions fail compilation.
- Division by zero fails compilation for compile-time-evaluated expressions.
- Unknown references fail compilation unless they are deferred layout-node cases above.
- `animation.width`/`animation.height` resolved numeric values must be `> 0`.
- Scalar values must be finite where validated.
- Layout node dimension expression dependencies must be acyclic.

### Example (chat mask growth)

```text
base = chat_header.height + viewport_base_padding
k1: base + msg_row_0.height
k2: base + msg_row_0.height + msg_row_1.height
k3: base + msg_row_0.height + msg_row_1.height + msg_row_2.height
```

Each keyframe can animate from the previously resolved height to the next expression-derived height.

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
