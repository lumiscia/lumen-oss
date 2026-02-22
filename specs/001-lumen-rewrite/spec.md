# Feature Specification: Lumen Rendering Engine Rewrite

**Feature Branch**: `001-lumen-rewrite`
**Created**: 2026-02-22
**Status**: Draft

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Clip Geometry and Positioning (Priority: P1)

A developer places any clip type in a scene at an explicit (x, y) coordinate with explicit width and height. An anchor point, expressed as a fraction of the clip's own bounds (0.0 = left/top edge, 0.5 = center, 1.0 = right/bottom edge), controls which point on the clip coincides with its declared position and serves as the pivot for all transforms. The canvas translates by (x, y) before drawing any clip content, and every clip type uses these values — no clip draws at debug-hardcoded positions or frame-relative percentages.

**Why this priority**: Without explicit positioning, no clip can be placed correctly. It is the prerequisite for every other visual feature. Currently every clip uses hardcoded frame-percentage positions (e.g., `frame_ctx.width * 0.1`), which produces only debug output.

**Independent Test**: Place two rectangles — one at (50, 50) with anchor (0.0, 0.0) and one at (200, 200) with anchor (0.5, 0.5). Render. Assert the first rectangle's top-left pixel is at canvas coordinate (50, 50); assert the second rectangle's center pixel is at (200, 200).

**Acceptance Scenarios**:

1. **Given** a `ClipGeometry` with `x: 100`, `y: 80`, `anchor_x: 0.0`, `anchor_y: 0.0`, **When** the clip is rendered, **Then** the clip's top-left corner is at canvas pixel (100, 80).
2. **Given** `anchor_x: 0.5`, `anchor_y: 0.5`, `x: 200`, `y: 150`, **When** the clip is rendered, **Then** the clip's center pixel is at canvas coordinate (200, 150).
3. **Given** `anchor_x: 1.0`, `anchor_y: 1.0`, **When** a rotation transform is applied, **Then** the clip rotates around its bottom-right corner, not its top-left.
4. **Given** a layout clip whose Taffy output overrides child positions, **When** children are rendered, **Then** Taffy-computed `location.x` and `location.y` replace the child's declared `x`/`y` for that render.
5. **Given** a clip positioned so that part of it is outside the frame bounds, **When** rendered, **Then** the visible portion is drawn correctly and the out-of-bounds portion is clipped without error.
6. **Given** all clip types (Group, Layout, Shape, Text, Image, Video), **When** each is placed at an explicit coordinate, **Then** each renders at that coordinate — no clip uses hardcoded or frame-relative positioning.

---

### User Story 2 — Fill and Stroke System (Priority: P1)

A developer configures any shape clip's visual appearance using a composable `Fill` and `Stroke`. Fills cover four modes: solid color (per-channel RGBA, each independently animatable), linear gradient (two control points with a stop list), radial gradient (center + radius with a stop list), and image fill (source ID with fit mode). A stroke adds an independently colored, width-configurable border with cap, join, and optional dash-pattern controls. A shape can be fill-only, stroke-only, or both. No clip ever renders with a hardcoded debug color.

**Why this priority**: Fill and stroke turn the existing debug-colored placeholders into actual rendered content. Without this, shapes are useless for production output.

**Independent Test**: Render a rectangle with a red-to-blue linear gradient fill (left to right) and a 3 px dashed green stroke with `LineCap::Round`. Assert: left-edge pixels are near red, right-edge pixels are near blue, the border pixels are green.

**Acceptance Scenarios**:

1. **Given** `Fill::Solid { color: [255, 0, 0, 255] }`, **When** rendered, **Then** the interior pixels of the shape are opaque red.
2. **Given** `Fill::LinearGradient { start: (0.0, 0.5), end: (1.0, 0.5), stops: [(0.0, red), (1.0, blue)] }`, **When** rendered, **Then** pixels interpolate smoothly from red to blue horizontally.
3. **Given** `Fill::RadialGradient { center: (0.5, 0.5), radius: 0.5, stops: [(0.0, white), (1.0, black)] }`, **When** rendered, **Then** the center is white and the outer edge is black.
4. **Given** `Fill::Image { source: "bg.png", fit: ImageFit::Cover }`, **When** rendered, **Then** the shape's interior shows the image scaled and cropped to fill the shape bounds.
5. **Given** `fill: None` and `stroke: Some(...)`, **When** rendered, **Then** only the stroke outline is visible; the shape interior is transparent.
6. **Given** `Stroke { width: 4.0, dash_pattern: Some([8.0, 4.0]), line_cap: LineCap::Butt, line_join: LineJoin::Miter }`, **When** rendered, **Then** the border shows a dashed pattern with the correct dash-gap rhythm.
7. **Given** a gradient stop whose `position` property is an animated `StyleProperty` keyframe sequence, **When** resolved at different frames, **Then** the stop position changes per-frame, producing an animated gradient shift.
8. **Given** `fill: Some(...)` and `stroke: Some(...)`, **When** rendered, **Then** the fill is drawn first (underneath) and the stroke is drawn on top, centered on the shape boundary.

---

### User Story 3 — Border Radius and Base-Style Clip Radius (Priority: P1)

A developer adds animatable per-corner border radius to a rectangle shape, causing it to render with rounded corners. Additionally, a `clip_radius` on `BaseStyle` applies a rounded rectangular clip mask to *any* clip type (including images and videos), so content is masked to the rounded bounds before compositing.

**Why this priority**: Border radius is one of the most-used visual properties in modern UIs. It transforms the existing `draw_rect` call into a real UI element. The base-style `clip_radius` generalizes the same capability to images and video, avoiding the need for a separate mask for the common case.

**Independent Test**: Render a 200×200 rectangle with corner radii [0, 50, 50, 0] (left side sharp, right side rounded). Assert: the top-left and bottom-left corners contain opaque pixels; the top-right and bottom-right corners contain fully transparent pixels at the extreme corners.

**Acceptance Scenarios**:

1. **Given** `corner_radius: [0.0, 0.0, 0.0, 0.0]`, **When** rendered, **Then** the rectangle has sharp corners identical to a plain `draw_rect`.
2. **Given** `corner_radius: [20.0, 20.0, 20.0, 20.0]` (uniform), **When** rendered, **Then** all four corners are visually rounded and anti-aliased.
3. **Given** `corner_radius: [0.0, 40.0, 40.0, 0.0]` (right corners only), **When** rendered, **Then** only the right two corners are rounded; the left corners remain sharp.
4. **Given** each corner radius value is a separate `StyleProperty<f32>` with its own keyframe sequence, **When** resolved at different frames, **Then** the corner radii animate independently.
5. **Given** `BaseStyle::clip_radius: [12.0, 12.0, 12.0, 12.0]` on an `ImageClip`, **When** rendered, **Then** the image is composited with rounded corners; pixels outside the rounded rect are transparent.
6. **Given** `clip_radius` applied to a `VideoClip`, **When** rendered, **Then** video frames are masked to the rounded rectangular bounds each frame.

---

### User Story 4 — Full 2D Transform System (Priority: P1)

A developer specifies independent per-axis translate, scale, rotation in degrees, and skew for any clip, along with a fractional transform origin. Transforms are applied in CSS-compatible order: translate to origin → apply translation → apply rotation → apply scale → apply skew → translate back from origin. Every component is an independently animatable `StyleProperty<f32>`, allowing any combination to be keyframed separately.

**Why this priority**: The current transform system has a single-axis translate (same value for both x and y), uniform scale, and no rotation or skew. It is non-functional for any real use case. Transforms are required before any animation work is meaningful.

**Independent Test**: Place a 100×100 rectangle at (200, 200) with origin (0.5, 0.5), rotate 90°, scale (2.0, 1.0). Assert the rendered output matches a reference showing the rectangle doubled in width and rotated 90° about its center.

**Acceptance Scenarios**:

1. **Given** `translate: [30.0, 0.0]` and all other transforms at identity, **When** rendered, **Then** the clip is offset 30 px to the right of its declared position.
2. **Given** `translate: [0.0, 50.0]`, **When** rendered, **Then** the clip is offset 50 px downward only, with no horizontal shift.
3. **Given** `scale: [2.0, 1.0]`, **When** rendered, **Then** the clip's width is doubled but its height is unchanged.
4. **Given** `rotation: 45.0` with `origin: [0.5, 0.5]`, **When** rendered, **Then** the clip rotates 45° clockwise about its center; the center position is unchanged.
5. **Given** `rotation: 45.0` with `origin: [0.0, 0.0]`, **When** rendered, **Then** the clip rotates 45° clockwise about its top-left corner.
6. **Given** `skew: [15.0, 0.0]`, **When** rendered, **Then** the clip is sheared horizontally by 15°.
7. **Given** `translate`, `rotation`, and `scale` all have separate keyframe sequences, **When** resolved at different frames, **Then** each component animates independently without affecting the others.
8. **Given** the transform order is (origin translate → user translate → rotate → scale → skew → origin detranslate), **When** multiple transforms are combined, **Then** the result matches the CSS transform matrix equivalent.

---

### User Story 5 — Keyframe Interpolation with Easing (Priority: P1)

A developer defines any `StyleProperty<T>` as a `Sequence` of keyframes, where each keyframe carries a frame index, a value, and an easing specification. At any given frame, the engine finds the two surrounding keyframes, applies the easing function to compute a normalized `t`, and calls `lerp` on the value type. Before the first keyframe the first value is held; after the last keyframe the last value is held. All animatable types (`f32`, `u8`, `u32`, `bool`) have defined interpolation behavior.

**Why this priority**: Keyframe animation is the engine's core purpose. The current sequence resolution returns the last literal keyframe unconditionally, making animation non-functional.

**Independent Test**: Create an `f32` property with keyframes at frame 0 (0.0) and frame 60 (600.0) with `CubicBezier(0.42, 0.0, 0.58, 1.0)` (standard ease-in-out). Evaluate at frames 10, 30, 50. Assert values fall on the expected cubic-bezier curve within ±0.05 of reference values.

**Acceptance Scenarios**:

1. **Given** two `f32` keyframes with `Linear` easing, **When** resolved at the exact midpoint frame, **Then** the result equals `(v1 + v2) / 2`.
2. **Given** `EaseIn` easing, **When** resolved at 25 % progress, **Then** the value is less than the linear 25 % value (slow start).
3. **Given** `EaseOut` easing, **When** resolved at 75 % progress, **Then** the value is greater than the linear 75 % value (fast end).
4. **Given** `CubicBezier(0.25, 0.1, 0.25, 1.0)`, **When** resolved at multiple frames, **Then** the output curve matches standard CSS cubic-bezier approximation within ±0.001.
5. **Given** `Steps(4, StepPosition::End)`, **When** resolved across the keyframe interval, **Then** the value steps in exactly 4 equal jumps at 25 %, 50 %, 75 %, 100 % of the interval.
6. **Given** a frame exactly on a keyframe boundary, **When** resolved, **Then** that keyframe's exact value is returned with no interpolation artifact.
7. **Given** a `u8` color channel property, **When** interpolated, **Then** the result is rounded to the nearest integer with no overflow.
8. **Given** a `bool` property with two keyframes, **When** resolved at t < 0.5, **Then** the first value is returned; at t ≥ 0.5, the second value is returned.
9. **Given** a single-keyframe sequence, **When** resolved at any frame, **Then** that keyframe's value is always returned.
10. **Given** a frame before the first keyframe, **When** resolved, **Then** the first keyframe's value is returned with no extrapolation.

---

### User Story 6 — Style Context Threading (Priority: P1)

Every style property resolution call receives a `StyleContext` carrying the current frame number and an optional `ExpressionScope`. This context is passed through `BaseStyle::resolve`, `BaseStyle::draw`, all shape draw methods, and all clip draw implementations. No call site passes a bare value or constant `0` as the frame. Switching from the current frame-unaware API to the context-bearing API does not require each clip to independently manage frame state.

**Why this priority**: Without context threading, keyframe interpolation and expression evaluation cannot reach property resolution. It is a prerequisite for both animation (US5) and expressions (US9). It also normalizes the API surface before any new features are added.

**Independent Test**: Construct a `StyleContext { frame: 15, scope: None }`. Resolve a `Sequence<f32>` property with two keyframes (frame 0 → 0.0, frame 30 → 30.0) using this context. Assert the result is `15.0`.

**Acceptance Scenarios**:

1. **Given** a `StyleContext` with `frame: 15`, **When** any `StyleProperty::resolve(ctx)` is called, **Then** the frame number is used for keyframe lookup without the caller needing to pass it separately.
2. **Given** a `StyleContext` with `scope: Some(expression_scope)`, **When** a `StyleValue::Expression` is encountered during resolution, **Then** the expression is evaluated against that scope.
3. **Given** `scope: None`, **When** a property holds an expression, **Then** resolution returns `None` (graceful fallback) without panicking.
4. **Given** a `BaseStyle::draw` call, **When** it invokes the draw callback, **Then** the same `StyleContext` is used for all nested property resolutions within that call.
5. **Given** the entire render pipeline for a frame, **When** it starts with a single `StyleContext { frame: N, scope: ... }`, **Then** that context reaches every `StyleProperty::resolve` call without any call site constructing its own frame value.

---

### User Story 7 — Text Rendering and Measurement (Priority: P1)

A developer places a `TextClip` configured with font family, size (animatable), weight (100–900), slant (Normal/Italic/Oblique), RGBA color (each channel independently animatable), line height multiplier, letter spacing in pixels, text alignment (Left/Center/Right/Justify), vertical alignment (Top/Middle/Bottom), optional max width (triggers wrapping), optional max line count with overflow mode (Clip/Ellipsis/Visible), and text decoration (None/Underline/Strikethrough). The rendered output shows correctly shaped glyphs. The engine also exposes an intrinsic measurement function that returns the paragraph's computed dimensions given an available layout width, for integration with Taffy layout.

**Why this priority**: Text is a core content type and a prerequisite for layout clips (which need text intrinsic sizing for Taffy measurement). The current implementation draws a white box with a baseline estimate.

**Independent Test**: Render "The quick brown fox" in 16 px Regular, left-aligned, `max_width: 80`. Assert the text wraps to at least 2 lines and the rendered bounding box height is greater than 16 px. Assert no pixels appear right of x=80.

**Acceptance Scenarios**:

1. **Given** `font_size: 24`, `color: [255, 0, 0, 255]`, `text_align: Left`, **When** rendered, **Then** glyphs appear left-aligned in red at approximately 24 px cap-height.
2. **Given** `text_align: Center`, `max_width: 200`, **When** rendered, **Then** each line is horizontally centered within the 200 px container.
3. **Given** `text_align: Justify` and a multi-word line, **When** rendered, **Then** word spacing expands so both edges of the line are flush with the container.
4. **Given** a string long enough to require wrapping and `max_width: 150`, **When** rendered, **Then** the text breaks into multiple lines, all within the 150 px width.
5. **Given** `max_lines: 2`, `overflow: Ellipsis`, and more than 2 lines of content, **When** rendered, **Then** the third and subsequent lines are hidden and line 2 ends with `…`.
6. **Given** `line_height: 1.5`, **When** rendered next to the same text at `line_height: 1.0`, **Then** the former has 50 % more spacing between baselines.
7. **Given** `letter_spacing: 4.0`, **When** rendered, **Then** each character has 4 additional pixels of spacing to its right compared to default.
8. **Given** `font_weight: 700`, **When** rendered, **Then** glyphs use the bold variant of the specified font family.
9. **Given** `font_style: Italic`, **When** rendered, **Then** glyphs use the italic variant.
10. **Given** `decoration: Underline`, **When** rendered, **Then** a line appears beneath the text baseline.
11. **Given** `vertical_align: Middle` and a text node taller than the text content, **When** rendered, **Then** the glyphs are vertically centered within the node's height.
12. **Given** a `TextClip` is queried for its intrinsic size with `available_width: 200.0`, **When** measured, **Then** the returned `(width, height)` matches the paragraph's computed `longest_line()` and `height()`.
13. **Given** `font_size` is a `Sequence` with keyframes at different frames, **When** the layout measure function is called, **Then** the current frame's resolved font size is used for measurement.
14. **Given** a font family that does not exist on the system, **When** rendered, **Then** the engine falls back to the system default font and renders successfully without panicking.

---

### User Story 8 — Masks (Priority: P2)

A developer applies a `Mask` to any clip via `BaseStyle`. Three mask sources are supported: a `Shape` mask (an inline geometric path — rectangle, ellipse, or arbitrary path — used as a clipping region), a `Bitmap` mask (an image whose alpha or luminance channel defines the mask), and a `Clip` mask (another clip's rendered alpha channel used as the mask). Masks can be inverted. The mask is applied during the base-style draw pass before the clip's own content is rendered.

**Why this priority**: Masks are a core compositing primitive needed for rounded image cutouts, vignettes, and dynamic shape masking. They depend on the base-style draw method pattern established in earlier work.

**Independent Test**: Apply a circular `Shape` mask (ellipse with equal rx/ry) to a solid red rectangle. Render. Assert that pixels inside the circle are red and pixels outside the circle are transparent.

**Acceptance Scenarios**:

1. **Given** `MaskSource::Shape(MaskShape::Rectangle { ... })` with `inverted: false`, **When** rendered, **Then** only pixels inside the rectangle mask region are visible.
2. **Given** the same rectangle mask with `inverted: true`, **When** rendered, **Then** only pixels outside the rectangle are visible; the interior is transparent.
3. **Given** `MaskSource::Shape(MaskShape::Ellipse { cx: 0.5, cy: 0.5, rx: 0.5, ry: 0.5 })`, **When** applied to a solid image clip, **Then** the image is clipped to a circle.
4. **Given** `MaskSource::Bitmap { source: "mask.png" }` where the image has a gradient alpha channel, **When** rendered, **Then** the clip content fades according to the mask image's alpha values.
5. **Given** `MaskSource::Clip { clip_id: "shape_layer" }` where the referenced clip is a white-filled shape on a transparent background, **When** rendered, **Then** the masked clip's content is only visible where the reference clip's alpha is non-zero.
6. **Given** a clip mask where the referenced clip has not yet been rendered (incorrect dependency order), **When** the dependency resolver is consulted, **Then** the referenced clip's render is added as a dependency, ensuring correct ordering.
7. **Given** `MaskShape::Rectangle` with animatable `x`, `y`, `width`, `height`, `corner_radius`, **When** resolved at different frames, **Then** the mask shape changes frame-by-frame.

---

### User Story 9 — Expression System (Priority: P2)

A developer assigns an expression string to any `StyleProperty`. The engine parses the string into an AST, extracts all clip and layout references, evaluates the expression at render time using a populated `ExpressionScope`, and uses the result as the property value. Expressions support numeric literals, arithmetic (+ − × ÷ mod), comparison (> < >= <= == !=), logical (and, or, not), conditional (if/then/else), clip property references (`clip('id').property`), layout node property references (`layout('id').property`), and built-in math functions (`min`, `max`, `abs`, `floor`, `ceil`, `round`, `clamp`, `lerp`, `sin`, `cos`).

**Why this priority**: Expressions enable reactive and data-driven layouts — a hallmark feature of the engine. They require the dependency resolver (US10) to already guarantee the expression scope is populated before evaluation.

**Independent Test**: Parse `"clip('header').height + 20"`. Assert the AST contains a `BinaryOp(Add, ClipRef('header', Height), Number(20))`. Evaluate with scope `{ ('header', Height): Number(80.0) }`. Assert result is `Number(100.0)`.

**Acceptance Scenarios**:

1. **Given** the expression `"100.0 + 50.0"`, **When** parsed and evaluated, **Then** the result is `Number(150.0)`.
2. **Given** `"clip('bg').width * 0.5"` with `bg.width = 400`, **When** evaluated, **Then** the result is `Number(200.0)`.
3. **Given** `"layout('sidebar').width"` with the layout node's computed width = 240, **When** evaluated, **Then** the result is `Number(240.0)`.
4. **Given** `"min(clip('a').width, clip('b').width)"` with widths 300 and 150, **When** evaluated, **Then** the result is `Number(150.0)`.
5. **Given** `"if(clip('toggle').opacity > 0, 100, 0)"` with toggle opacity = 1.0, **When** evaluated, **Then** the result is `Number(100.0)`.
6. **Given** an expression referencing a clip that is not in the scene, **When** evaluated, **Then** an `ExpressionError::UnresolvedReference` is returned with the unknown clip id.
7. **Given** `"clip('a').width + true"` (type mismatch), **When** evaluated, **Then** an `ExpressionError::TypeMismatch` is returned.
8. **Given** `"unknown_func(1, 2)"`, **When** evaluated, **Then** an `ExpressionError::UnknownFunction { name: "unknown_func" }` is returned.
9. **Given** the parser walks the AST after parsing, **When** it encounters `ClipRef` and `LayoutRef` nodes, **Then** those references are collected into `Expression.references` for use by the dependency resolver.
10. **Given** `"clamp(clip('x').width, 0, 500)"`, **When** evaluated with `width = 600`, **Then** the result is `Number(500.0)`.
11. **Given** `"-clip('a').width"` (unary negation), **When** evaluated with `width = 100`, **Then** the result is `Number(-100.0)`.
12. **Given** `"not (clip('a').opacity > 0)"`, **When** evaluated with opacity = 0, **Then** the result is `Boolean(true)`.

---

### User Story 10 — Dependency Resolution and Render Ordering (Priority: P2)

The engine collects all expressions from all clips in the scene, builds a directed dependency graph where each expression's referenced clips are prerequisites, topologically sorts the graph using Kahn's algorithm, and executes render/compute operations in that order. Computed property values (x, y, width, height, opacity) are stored in a `ResultMap` after each clip renders, making them available to subsequent expression evaluations. Circular dependencies produce a descriptive error with the full cycle path.

**Why this priority**: Cross-clip expressions cannot function without a correct render order. This is also what makes group and layout dependencies (children before parent) formally correct.

**Independent Test**: Scene with clip A (static width=400), clip B whose x-position expression is `clip('A').width * 0.5`, clip C whose y-position expression is `clip('B').x + 10`. Assert topological order is [A, evaluate B.x expr, B, evaluate C.y expr, C].

**Acceptance Scenarios**:

1. **Given** clip B depends on clip A's width, **When** the dependency graph is sorted, **Then** clip A's render node appears before clip B's render node.
2. **Given** a diamond dependency (clips C and D both depend on clip A, and clip E depends on both C and D), **When** sorted, **Then** clip A appears before C and D, and both C and D appear before E.
3. **Given** clip A depends on clip B's height and clip B depends on clip A's width (circular), **When** the graph is sorted, **Then** a `DependencyTreeError::Cycle` is returned naming both clip A and clip B in the cycle path.
4. **Given** a group clip whose children are C and D, **When** an expression references `clip('group').width`, **Then** C and D must render before the group's bounding-box computation, which must complete before the expression is evaluated.
5. **Given** a layout clip contains a text node whose `font_size` is an expression, **When** the render order is determined, **Then** the font-size expression evaluates before the layout computation, because layout measurement depends on font size.
6. **Given** a `LayoutCompute` node for layout "sidebar", **When** an expression references `layout('sidebar').width`, **Then** the `LayoutCompute` node for "sidebar" appears before the expression evaluation in the topological order.
7. **Given** a scene with 50 clips and no cyclic dependencies, **When** the dependency plan is built, **Then** `DependencyPlan::evaluation_order` contains exactly as many entries as there are render + expression + layout nodes, and each appears exactly once.
8. **Given** a clip with no expressions in its style, **When** the dependency graph is built, **Then** that clip has no incoming expression edges (it can render in any order relative to non-dependent clips).

---

### User Story 11 — Layout Clips with Content Nodes (Priority: P2)

A developer creates a `LayoutClip` where each `LayoutNode` optionally holds a `LayoutContent` — one of: `Text(TextClip)`, `Image(ImageClip)`, `Video(VideoClip)`, `Shape(ShapeClip)`, or `Layout(LayoutClip)`. After Taffy computes the layout, the engine traverses the node tree and renders each node's content at the Taffy-computed position (offset from the layout clip's own origin) and at the Taffy-computed size. Text nodes register a Taffy measure function that invokes the text intrinsic measurement API so Taffy can size text nodes correctly. The current debug-only rendering (purple outline + green circle) is entirely replaced.

**Why this priority**: Layout clips are required for document-like UIs and structured compositions. They depend on text measurement (US7) and clip positioning (US1) being complete.

**Independent Test**: A 400×200 horizontal flex layout containing a `Shape(rect, fill: red, flex_grow: 1)` and a `Text("Hi", flex_grow: 1)`. Render. Assert the rectangle occupies pixels 0–199 and the text occupies pixels 200–399 horizontally, both at full height.

**Acceptance Scenarios**:

1. **Given** a flex layout with two equal `flex_grow: 1` children, each 400 px wide parent, **When** rendered, **Then** each child is positioned and sized at exactly 200 px wide.
2. **Given** a `LayoutContent::Text` node, **When** Taffy measures the layout, **Then** the text's intrinsic size (from paragraph measurement) is used to compute the node's dimensions.
3. **Given** `justify_content: SpaceBetween` and three children, **When** rendered, **Then** the first child is flush left, the last is flush right, and the middle child is centered with equal spacing.
4. **Given** `align_items: Center` in a row direction, **When** children have different heights, **Then** each child is vertically centered within the row.
5. **Given** a child with `position: Absolute`, `inset: { top: 8, right: 8 }`, **When** rendered, **Then** the child appears 8 px from the top and right edges of the layout container.
6. **Given** `overflow: Hidden` on the layout container and a child that overflows its bounds, **When** rendered, **Then** the overflowing portion of the child is clipped at the container boundary.
7. **Given** a `LayoutContent::Shape(ShapeClip)` node with `fill: Solid(blue)`, **When** rendered, **Then** a solid blue rectangle appears at the Taffy-computed position and size.
8. **Given** nested `LayoutContent::Layout(LayoutClip)` (a flex inside a flex), **When** rendered, **Then** the inner layout correctly positions its children relative to the inner layout's origin.
9. **Given** `display: Grid` with `grid_template_columns: [1fr, 1fr]`, **When** rendered with four child nodes, **Then** children are arranged in a 2×2 grid at correct positions.
10. **Given** a layout node with `aspect_ratio: 16.0 / 9.0` and a flex-driven width, **When** rendered, **Then** the height is automatically computed to maintain the 16:9 ratio.

---

### User Story 12 — Image Rendering with Fit Modes (Priority: P2)

A developer places an `ImageClip` with a source ID and an `ImageFit` mode. The engine decodes the image to RGBA pixels via the `ImageResolver` trait, creates a cached drawable representation, and renders it to the canvas at the clip's declared bounds using the specified fit mode. The fit mode controls how the source image's aspect ratio is reconciled with the clip's bounds. A per-source-ID cache prevents re-decoding on every frame.

**Why this priority**: Image rendering produces real visual output for one of the most common clip types. The current implementation decodes dimensions but then draws a colored rectangle instead of actual pixels.

**Independent Test**: Provide a 200×100 (2:1 landscape) image. Render it into a 100×100 square clip with each of the four fit modes. Assert pixel layouts match expected: Cover = cropped center, Contain = letterboxed with empty strips top/bottom, Fill = stretched, None = original size with right half clipped.

**Acceptance Scenarios**:

1. **Given** `ImageFit::Cover` and a source image wider than the clip, **When** rendered, **Then** the image fills the clip completely; the left and right edges overflow and are cropped.
2. **Given** `ImageFit::Contain` and a source image wider than the clip, **When** rendered, **Then** the image is scaled to fit the width; empty bars appear above and below.
3. **Given** `ImageFit::Fill`, **When** rendered, **Then** the image is stretched to exactly match the clip's width and height regardless of aspect ratio.
4. **Given** `ImageFit::None`, **When** the source image is larger than the clip, **Then** the image is drawn at original size and clipped to the clip's bounds.
5. **Given** the same image source is used by two clips in the same frame, **When** rendered, **Then** the image is decoded only once; the second clip uses the cached representation.
6. **Given** a source ID that does not exist in the `MediaStore`, **When** rendered, **Then** a `RenderError::MissingSource` is returned containing the source ID.
7. **Given** a valid image source, **When** the rendered pixels are read back, **Then** they match the source pixel data (within RGBA precision) at the expected position.

---

### User Story 13 — Video Rendering with Timeline Mapping (Priority: P2)

A developer places a `VideoClip` with a source ID, optional trim range (in seconds), playback speed multiplier, and loop mode. For each timeline frame, the engine maps the timeline frame to the correct source video frame using PTS-based arithmetic that accounts for trim, speed, and loop, requests that frame from the decoder, and renders the decoded RGBA pixels to the canvas. The video clip supports `LoopMode::None` (hold last frame), `LoopMode::Repeat` (restart), and `LoopMode::PingPong` (reverse at end). The feature is gated behind the `ffmpeg` cargo feature.

**Why this priority**: Video is the primary content type in the engine's target domain (video composition). Correct frame mapping is critical; an off-by-one in PTS conversion produces visibly wrong output.

**Independent Test**: Create a `VideoClip` with `trim: 2.0..6.0`, `speed: 2.0`, `LoopMode::Repeat` at 30 fps. At timeline frame 0, assert the requested source frame equals frame 60 (2.0 s × 30 fps = frame 60). At timeline frame 60 (2.0 s of wall time), assert the source frame equals frame 120 (4.0 s into source, still within trim). At timeline frame 90, the 4.0 s of wall time × speed 2.0 = 8.0 s of source time, which exceeds the 4.0 s trim duration; with `Repeat`, assert source frame wraps to frame 60.

**Acceptance Scenarios**:

1. **Given** `trim: None`, `speed: 1.0`, `LoopMode::None`, **When** at timeline frame N, **Then** the requested source frame is N.
2. **Given** `trim: 1.0..5.0` (30 fps), **When** at timeline frame 0, **Then** the requested source frame is 30 (1.0 s × 30 fps).
3. **Given** `speed: 2.0`, **When** at timeline frame 30, **Then** the requested source frame is 60 (2× speed).
4. **Given** `LoopMode::Repeat` and the speed-adjusted frame exceeds the trim duration, **When** mapped, **Then** the source frame wraps modulo the trim duration.
5. **Given** `LoopMode::PingPong` and the frame is past the trim midpoint, **When** mapped, **Then** the source frame plays in reverse (trim_end − offset).
6. **Given** `LoopMode::None` and the speed-adjusted frame exceeds the trim end, **When** mapped, **Then** the last frame of the trim range is returned (hold last frame).
7. **Given** the source video's native frame rate differs from the timeline frame rate, **When** PTS-mapped, **Then** the nearest source frame is decoded without visible judder on clean sequential playback.
8. **Given** a decoded RGBA frame, **When** rendered to the canvas, **Then** the pixels appear at the clip's declared position with correct `ImageFit` mode applied.

---

### User Story 14 — Video Decode Pipeline (FFmpeg, Feature-Gated) (Priority: P2)

The engine includes a `LibavStreamDecoder` that wraps FFmpeg's `libav*` libraries to decode video from any container/codec that FFmpeg supports. The decoder converts decoded frames to RGBA using `swscale`, caches frames in a bounded LRU cache, recycles allocation buffers, supports seeking (keyframe seek with reopen fallback for non-seekable sources), and optionally uses hardware-accelerated decoding (VideoToolbox on macOS, VAAPI/CUDA on Linux, D3D11VA on Windows) with graceful fallback to software decode. FFmpeg is initialized globally once via a `OnceLock`.

**Why this priority**: The decode pipeline is the most complex subsystem. It must be correct (right frame at right time) and efficient (LRU cache avoids redundant decodes) for the engine to be usable for real video editing.

**Independent Test**: Open a known test video, request frame 0, then frame 100, then frame 50 (a backward seek). Assert all three frames return the correct RGBA pixel data (verified against a reference decoder). Assert the LRU cache contains frame 100 after the second call.

**Acceptance Scenarios**:

1. **Given** a sequential forward request (frame N, then frame N+1), **When** frame N+1 is requested after N, **Then** it is served from the LRU cache without a seek operation.
2. **Given** a backward seek (request frame 50 after frame 100), **When** the decoder processes it, **Then** a keyframe seek is issued to the nearest keyframe before frame 50, then frames are decoded forward to frame 50.
3. **Given** a non-seekable source, **When** a backward seek is requested, **Then** the decoder reopens the source from the beginning and decodes forward to the target frame.
4. **Given** the source video's native pixel format is YUV420P, **When** decoded, **Then** the returned RGBA bytes match the correctly converted color values.
5. **Given** `LUMEN_LIBAV_HW_DEVICE=auto`, **When** the decoder initializes, **Then** it attempts hardware decode with platform-appropriate device types in order; if all fail, it falls back to software decode without error.
6. **Given** the LRU cache is at capacity (default 64 frames), **When** a new frame is decoded, **Then** the least-recently-used frame is evicted; its backing buffer is recycled into the buffer pool if no other reference holds it.
7. **Given** a source video with lower frame rate than the timeline, **When** multiple consecutive timeline frames map to the same source frame, **Then** the source frame is decoded once and the same data is returned for all mapping timeline frames.
8. **Given** `LUMEN_STREAM_CACHE_FRAMES=128` in the environment, **When** the decoder is initialized, **Then** the LRU cache capacity is 128 frames.
9. **Given** FFmpeg has not been initialized, **When** `LibavStreamDecoder::new` is called, **Then** `ffmpeg::init()` is called exactly once (subsequent decoders share the initialization).

---

### User Story 15 — Per-Source Decode Worker Threads with Prefetch (Priority: P2)

Each video source gets a dedicated background thread that owns the `LibavStreamDecoder` for that source. The render thread communicates with worker threads via bounded request channels. After serving a frame request, the worker detects sequential access patterns (forward or reverse) and pre-decodes the next N frames into its LRU cache. The render thread is never blocked by decoding when prefetch has populated the cache in advance.

**Why this priority**: Single-threaded sequential decoding would block the render thread while FFmpeg decodes, capping throughput to one frame per decode time. Background workers with prefetch decouple decoding from rendering.

**Independent Test**: Instantiate a worker for a test video. Request frame 10. Then request frame 11. Assert frame 12 through frame 10+prefetch_count are already in the worker's LRU cache before the frame 11 reply is received (inspect cache state via test hook).

**Acceptance Scenarios**:

1. **Given** sequential forward access (frame N, N+1, N+2, ...), **When** frame N is requested, **Then** the worker pre-decodes frames N+1 through N+prefetch_frames into the cache before the next request arrives.
2. **Given** sequential reverse access (frame N, N-1, N-2, ...), **When** frame N is requested, **Then** the worker detects reverse direction and pre-decodes frames N-1 through N-prefetch_frames into the cache.
3. **Given** random access (non-sequential requests), **When** a frame is requested, **Then** no prefetch is triggered; the worker decodes only the requested frame.
4. **Given** a `PingPong` video clip approaching the reversal point, **When** the render thread sends requests that cross from forward to reverse, **Then** the worker smoothly transitions to reverse prefetch.
5. **Given** the render thread drops the worker's sender channel, **When** the worker's receive loop processes the disconnect, **Then** the worker thread exits cleanly and all resources (decoder, LRU cache) are freed.
6. **Given** `LUMEN_LIBAV_PREFETCH_FRAMES=8`, **When** forward sequential access is detected, **Then** 8 frames are pre-decoded after each request.
7. **Given** the decode channel is bounded at `LUMEN_LIBAV_PREFETCH_QUEUE=8` requests, **When** the render thread sends requests faster than the worker can serve them, **Then** the render thread blocks on the channel send, providing natural backpressure.
8. **Given** scene preparation before the render loop, **When** `prepare_assets` is called, **Then** one worker thread per unique video source is spawned; no worker threads exist for scenes with no video clips.

---

### User Story 16 — Shadow Rendering (Priority: P3)

A developer configures any number of shadows on a clip via `BaseStyle::shadows: Vec<ShadowStyle>`. Each shadow specifies offset (x, y), Gaussian blur radius, spread, color (RGBA, each channel independently animatable), and an `inset` flag. The blur is implemented as a true Gaussian blur, not an opacity approximation. Inset shadows appear inside the clip's bounds. Multiple shadows render in declaration order (first = bottommost). All shadow properties are animatable.

**Why this priority**: Shadows are a common visual polish element. The current implementation fakes blur with opacity division, producing incorrect results. Fixing this requires the base-style draw method established earlier.

**Independent Test**: Render a white rectangle with one shadow: offset (0, 4), blur 8, color black, no inset. Render another with the same shadow as inset. Assert: the outer shadow's dark pixels appear below the rectangle; the inner shadow's dark pixels appear inside the rectangle at the top edge.

**Acceptance Scenarios**:

1. **Given** `offset_x: 4, offset_y: 4, blur: 0, color: black`, **When** rendered, **Then** a sharp offset shadow appears below-right of the clip with no blurring.
2. **Given** `blur: 10`, **When** rendered, **Then** the shadow edges are visibly soft; pixel intensity falls off with a Gaussian distribution.
3. **Given** `spread: 5` (positive), **When** rendered, **Then** the shadow shape is 5 px larger than the clip's bounds before blurring.
4. **Given** `spread: -5` (negative), **When** rendered, **Then** the shadow shape is 5 px smaller than the clip's bounds before blurring.
5. **Given** `inset: true`, **When** rendered, **Then** the dark shadow pixels appear inside the clip's boundary rather than outside.
6. **Given** `shadows: [shadow_A, shadow_B]` (two shadows), **When** rendered, **Then** shadow_A appears below shadow_B in the composited output (first declaration = bottommost).
7. **Given** `shadows: []` (empty list), **When** rendered, **Then** no shadow-related pixels appear and no error is raised.
8. **Given** the shadow `color` channels are each separate `StyleProperty<u8>` with keyframe sequences, **When** resolved at different frames, **Then** the shadow color animates independently per channel.

---

### User Story 17 — Scene and Layer Model (Priority: P3)

A developer defines a `Scene` as the root container specifying canvas dimensions, frame rate, and total duration. The scene contains an ordered list of `Layer` objects. Each layer holds its own list of clips, a `blend_mode`, an animatable `opacity`, and a `visible` flag. Layers render bottom-to-top. Each layer is composited as a unit onto the scene canvas using its blend mode and opacity. Invisible layers (`visible: false`) produce no output. The layer/scene model is the top-level entry point for the render pipeline.

**Why this priority**: The scene/layer model is the formal architectural container for all other rendering work. Without it, the render pipeline has no structured root.

**Independent Test**: Scene with two layers: layer 0 (red full-opacity rectangle), layer 1 (`BlendMode::Screen`, 80 % opacity, blue rectangle overlapping layer 0). Render. Assert that overlapping pixels show a screen-blended mix of red and blue at 80 % layer opacity; non-overlapping portions of layer 0 are unaffected.

**Acceptance Scenarios**:

1. **Given** two layers with default `BlendMode::Normal`, **When** rendered, **Then** layer 1 (top) paints over layer 0 in overlapping regions.
2. **Given** `BlendMode::Multiply` on layer 1, **When** rendered, **Then** overlapping pixel values equal the multiply blend formula applied to the two layer pixel values.
3. **Given** `visible: false` on a layer, **When** rendered, **Then** zero pixels from that layer appear in the output; layers above and below are unaffected.
4. **Given** layer opacity is an animated `StyleProperty<f32>` with keyframes 0.0→1.0, **When** rendered at different frames, **Then** the layer fades in progressively.
5. **Given** a scene with `frame_rate: Rational(24, 1)`, **When** `FrameContext` is constructed for frame 48, **Then** `time_seconds` equals 2.0.
6. **Given** a `Scene` with `duration_frames: 300` (10 s at 30 fps), **When** asked to render frame 301, **Then** the engine returns an out-of-range error rather than silently producing garbage output.

---

### Edge Cases

- **Zero-size clip**: A clip with `width: 0` or `height: 0` renders nothing and returns `Ok(())` without error.
- **Expression references nonexistent clip**: Returns `ExpressionError::UnresolvedReference { clip_id }` immediately; the render does not proceed for the affected clip.
- **Video source frame rate mismatch**: PTS arithmetic maps timeline frames to source frames; when multiple timeline frames map to the same source frame, the decoder returns the same cached frame for all — no seek is issued.
- **Missing font family**: Falls back to the system default font manager's default face; text renders (possibly with wrong style) rather than crashing.
- **Single-keyframe sequence**: The sole keyframe's value is returned at every frame regardless of the current frame number.
- **GPU surface creation failure**: The backend falls back to the software CPU raster backend; the failure is logged but does not propagate as a render error unless software init also fails.
- **LoopMode::None at trim end**: `map_to_source_frame` returns `trim_start + trim_duration - 1` for any speed-adjusted frame that would exceed the trim range; the last frame is held indefinitely.
- **Bitmap mask source not in MediaStore**: Returns `RenderError::MissingSource` with the mask's source ID.
- **Clip mask references a clip that renders after the masked clip in dependency order**: The dependency resolver adds an ordering edge to ensure the mask source clip renders first; if this creates a cycle, `DependencyTreeError::Cycle` is returned.
- **Gradient with zero stops**: Treated as transparent fill (no gradient is drawn); no panic.
- **`Steps(0, ...)` easing**: Treated as `Steps(1, ...)` (snap immediately); no divide-by-zero.
- **Expression result type mismatch**: If an expression resolves to a `Boolean` but the property expects `f32`, the engine returns `None` (graceful fallback) and logs a type mismatch warning.
- **`max_lines: 0` on text**: Treated as unlimited lines (same as `None`).
- **LRU cache frame count of 0**: Decoder operates with no caching; every request triggers a decode or seek. Allowed but discouraged; logged as a warning.

---

## Requirements *(mandatory)*

### Functional Requirements

#### Clip Geometry

- **FR-001**: Every clip type MUST declare spatial properties — `x`, `y`, `width`, `height`, `anchor_x`, `anchor_y` — each as an independently animatable `StyleProperty<f32>`. No clip may use frame-relative or hardcoded positioning.
- **FR-002**: `anchor_x` and `anchor_y` MUST be fractional values in the range [0.0, 1.0] representing the clip's own bounds: `0.0` = left/top edge, `0.5` = center, `1.0` = right/bottom edge. The anchor point is the canvas coordinate that coincides with the declared `x`/`y`.
- **FR-003**: When a clip is a child of a `LayoutClip`, the Taffy-computed `location.x` and `location.y` MUST override the clip's declared `x`/`y` for that render pass.

#### Fill, Stroke, and Shape Rendering

- **FR-004**: Shape clips (rectangle, ellipse, polygon) MUST support `fill: Option<Fill>` and `stroke: Option<Stroke>`, where either or both may be present. A shape with neither renders nothing.
- **FR-005**: `Fill` MUST be one of: `Solid { color: [StyleProperty<u8>; 4] }`, `LinearGradient { start: [StyleProperty<f32>; 2], end: [StyleProperty<f32>; 2], stops: Vec<GradientStop> }`, `RadialGradient { center: [StyleProperty<f32>; 2], radius: StyleProperty<f32>, stops: Vec<GradientStop> }`, `Image { source: String, fit: ImageFit }`.
- **FR-006**: `GradientStop` MUST have `position: StyleProperty<f32>` (0.0–1.0) and `color: [StyleProperty<u8>; 4]` (RGBA), each channel independently animatable.
- **FR-007**: `Stroke` MUST have `color: [StyleProperty<u8>; 4]`, `width: StyleProperty<f32>`, `dash_pattern: Option<Vec<f32>>`, `line_cap: LineCap` (Butt / Round / Square), `line_join: LineJoin` (Miter / Round / Bevel).
- **FR-008**: Every multi-component property (RGBA color, gradient control points, corner radii, translation, scale, skew, transform origin) MUST be stored as `[StyleProperty<T>; N]` — never as `StyleProperty<[T; N]>` — so each component is independently animatable and expressable.
- **FR-009**: `RectStyle` MUST add `corner_radius: [StyleProperty<f32>; 4]` (top-left, top-right, bottom-right, bottom-left). When any resolved corner value is non-zero, the rectangle MUST render as a rounded rectangle.
- **FR-010**: `BaseStyle` MUST add `clip_radius: [StyleProperty<f32>; 4]` that clips any clip type's rendered output to a rounded rectangular region before compositing, using the same per-corner model as `corner_radius`.

#### Transforms

- **FR-011**: `TransformStyle` MUST include `translate: [StyleProperty<f32>; 2]` (x, y), `scale: [StyleProperty<f32>; 2]` (x, y), `rotation: StyleProperty<f32>` (degrees, clockwise), `skew: [StyleProperty<f32>; 2]` (x, y in degrees), `origin: [StyleProperty<f32>; 2]` (fractional, default [0.5, 0.5]).
- **FR-012**: Transforms MUST be applied in CSS-compatible order: (1) translate canvas to `origin * clip_size`, (2) apply `translate`, (3) apply `rotation`, (4) apply `scale`, (5) apply `skew`, (6) translate canvas back by `-origin * clip_size`.

#### Style Resolution

- **FR-013**: `StyleProperty<T>::resolve(ctx: &StyleContext) -> Option<T>` MUST accept a `StyleContext` containing the current frame number and an optional `ExpressionScope`. Callers MUST NOT pass a bare frame number or construct a context inline.
- **FR-014**: `StyleContext` MUST be passed by reference through `BaseStyle::resolve`, `BaseStyle::draw`, all shape-style draw methods, and all clip `draw` implementations. No call site may substitute a constant frame value.
- **FR-015**: For `StyleProperty::Sequence`, resolution MUST find the two keyframes bracketing the current frame, apply the first keyframe's easing to compute `t`, then call `T::lerp(a, b, t)`.
- **FR-016**: The `Interpolate` trait MUST be implemented for `f32` (linear), `u8` (rounded linear), `u32` (rounded linear), and `bool` (snap: if t < 0.5 return self, else return other).
- **FR-017**: Easing variants MUST include `Linear`, `EaseIn`, `EaseOut`, `EaseInOut`, `CubicBezier(f32, f32, f32, f32)` (CSS-spec control points), `Steps(u32, StepPosition)`. `CubicBezier` MUST use a numerical solver (bisection or Newton-Raphson) accurate to ±0.001.
- **FR-018**: If `scope` is `None` and a `StyleValue::Expression` is encountered, resolution MUST return `None` (graceful fallback). No panic is permitted.

#### API Conventions

- **FR-019**: All free functions whose first argument is effectively `&Self` MUST be converted to methods: `resolve_style_value` → `StyleProperty::resolve`, `resolve_style_value_or` → `StyleProperty::resolve_or`, `resolve_base_style` → `BaseStyle::resolve`, `draw_with_base_style` → `BaseStyle::draw`, shape draw helpers → `ShapeKind::draw` or per-style methods, `build_dependency_plan` → `DependencyPlan::build`, `parse_expression` → `Expression::parse`, `evaluate_expression` → `Expression::evaluate`.

#### Text Rendering

- **FR-020**: `TextStyle` MUST include: `font_family: String`, `font_size: StyleProperty<f32>`, `font_weight: StyleProperty<u32>` (100–900), `font_style: FontSlant` (Normal/Italic/Oblique), `color: [StyleProperty<u8>; 4]`, `line_height: StyleProperty<f32>` (multiplier), `letter_spacing: StyleProperty<f32>` (px), `text_align: TextAlign` (Left/Center/Right/Justify), `vertical_align: VerticalAlign` (Top/Middle/Bottom), `max_width: Option<StyleProperty<f32>>`, `max_lines: Option<u32>`, `overflow: TextOverflow` (Clip/Ellipsis/Visible), `decoration: TextDecoration` (None/Underline/Strikethrough).
- **FR-021**: Text rendering MUST use the `textlayout` Paragraph API (already enabled in `Cargo.toml`) for Unicode-correct glyph shaping, line breaking, and text metrics.
- **FR-022**: `TextClip` MUST expose a `measure(available_width: f32, ctx: &StyleContext) -> (f32, f32)` function returning `(longest_line, total_height)` for use as a Taffy measure function. The measurement MUST use the resolved font size at the given frame.
- **FR-023**: When a `TextClip` is a `LayoutContent` node in a `LayoutClip`, the layout tree MUST register a `MeasureFunc` for that Taffy node that delegates to `TextClip::measure`.

#### Layout Clips

- **FR-024**: `LayoutNode` MUST add `content: Option<LayoutContent>` alongside its existing `id`, `style` (Taffy `Style`), and `children`. `LayoutContent` MUST be an enum of `Text(TextClip)`, `Image(ImageClip)`, `Video(VideoClip)`, `Shape(ShapeClip)`, `Layout(LayoutClip)`.
- **FR-025**: The layout render pass MUST traverse the Taffy node tree after `compute_layout`, retrieve each node's `layout().location` and `layout().size`, and render `LayoutContent` clips at those computed bounds.
- **FR-026**: The layout system MUST support these Taffy `Style` fields: `display` (Flex/Grid/Block/None), `flex_direction`, `flex_wrap`, `justify_content`, `align_items`, `align_self`, `align_content`, `gap`, `padding`, `margin`, `border`, `width`/`height`/`min_width`/`min_height`/`max_width`/`max_height`, `flex_grow`/`flex_shrink`/`flex_basis`, `position` (Relative/Absolute), `inset`, `overflow`, `aspect_ratio`.

#### Masks

- **FR-027**: `BaseStyle` MUST add `mask: Option<Mask>`. `Mask` contains `source: MaskSource` and `inverted: bool`. `MaskSource` is one of `Shape(MaskShape)`, `Bitmap { source: String }`, `Clip { clip_id: String }`.
- **FR-028**: `MaskShape` MUST support `Rectangle { x, y, width, height, corner_radius: [StyleProperty<f32>; 4] }`, `Ellipse { cx, cy, rx, ry }`, and `Path { data: Vec<PathCommand> }`.
- **FR-029**: Shape masks MUST be applied via canvas clip path (Intersect when `inverted: false`, Difference when `inverted: true`) before the draw callback runs, so the clip state is restored when the canvas save/restore exits.
- **FR-030**: Bitmap masks MUST render the clip content to a temporary surface, render the mask image to another surface, then composite using `BlendMode::DstIn` (`inverted: false`) or `BlendMode::DstOut` (`inverted: true`).
- **FR-031**: Clip masks MUST use the referenced clip's rendered alpha channel as the mask, rendered to a temporary surface. The referenced clip MUST appear before the masked clip in the dependency order.

#### Expressions

- **FR-032**: `Expression` MUST hold a parsed AST and a `Vec<ExpressionReference>` populated by walking the AST. `Expression::parse(id, source)` MUST parse the string or return `ExpressionError::ParseError`.
- **FR-033**: The expression AST MUST support nodes: `Number(f32)`, `Boolean(bool)`, `String(String)`, `ClipRef { clip_id, property }`, `LayoutRef { node_id, property }`, `BinaryOp { op, left, right }`, `UnaryOp { op, expr }`, `FuncCall { name, args }`, `Conditional { condition, then, otherwise }`.
- **FR-034**: Binary operators MUST include: `Add`, `Sub`, `Mul`, `Div`, `Mod`, `Gt`, `Lt`, `Gte`, `Lte`, `Eq`, `Neq`, `And`, `Or`. Unary operators: `Neg`, `Not`.
- **FR-035**: Built-in functions MUST include: `min`, `max`, `abs`, `floor`, `ceil`, `round`, `clamp`, `lerp`, `sin`, `cos`. Calling an unknown function MUST return `ExpressionError::UnknownFunction { name }`.
- **FR-036**: `ExpressionProperty` MUST include at minimum: `X`, `Y`, `Width`, `Height`, `Opacity`.
- **FR-037**: `Expression::evaluate(&self, scope: &ExpressionScope) -> Result<ExpressionValue, ExpressionError>` MUST evaluate the AST. Type mismatches and unresolved references MUST return typed errors, not panics.

#### Dependency Resolution

- **FR-038**: `DependencyPlan::build(expressions: &[Expression]) -> Self` MUST collect all `ExpressionReference` entries from all expressions and build a directed graph.
- **FR-039**: `DependencyTree::topological_order` MUST implement Kahn's algorithm. If the graph has no cycles, it MUST return a correctly ordered `Vec<DependencyNode>`. If a cycle exists, it MUST return `DependencyTreeError::Cycle` with the cycle nodes identified.
- **FR-040**: `DependencyNode` MUST include `ClipRender { clip_id, layer }`, `LayoutCompute { layout_id }`, and `Expression(ExpressionId)` variants.
- **FR-041**: The render loop MUST maintain a `ResultMap: HashMap<(String, ExpressionProperty), ExpressionValue>` populated after each clip renders and each layout is computed. Expression evaluation MUST build its `ExpressionScope` from this map.
- **FR-042**: Group clips MUST add dependency edges from all children to the group's own render node, so the group's bounding box is computed after all children are rendered.
- **FR-043**: Layout clips with text nodes that have expression-driven style properties (e.g., animated `font_size`) MUST schedule those expression evaluations before the `LayoutCompute` node in the dependency order.

#### Image Rendering

- **FR-044**: `ImageClip::draw` MUST decode source pixels via `ImageResolver::resolve()`, create a drawable image from the raw RGBA bytes, and render it to the canvas at the clip's computed bounds using `ImageFit`.
- **FR-045**: `ImageFit` MUST support `Cover` (scale to fill, crop overflow), `Contain` (scale to fit, letterbox), `Fill` (stretch to exact clip bounds), `None` (original size, clip if larger).
- **FR-046**: The engine MUST cache the drawable representation of each image source by source ID on `RendererContext` or an associated cache. The cache is invalidated only when the source changes. Re-decoding MUST NOT occur on every frame for a static image source.

#### Video Rendering

- **FR-047**: `VideoClip::map_to_source_frame(timeline_frame: u32) -> Option<u64>` MUST apply: (1) subtract clip start, (2) multiply by `speed`, (3) add `trim_start` (frames), (4) apply `LoopMode`. Returns `None` if `trim_duration == 0`.
- **FR-048**: `LoopMode` MUST implement: `None` → clamp to `trim_end - 1`; `Repeat` → modulo `trim_duration`; `PingPong` → `cycle = 2 * trim_duration`; `pos = speed_adjusted % cycle`; if `pos < trim_duration`, use `pos`; else use `cycle - pos - 1`.

#### Video Decode Pipeline (feature = "ffmpeg")

- **FR-049**: `LibavStreamDecoder` MUST convert decoded frames from their native pixel format to RGBA using `ffmpeg::software::scaling::Context` with `FAST_BILINEAR` flags. Stride padding MUST be handled correctly.
- **FR-050**: `LibavStreamDecoder` MUST implement `source_frame_to_pts` and `pts_to_source_frame` using the source stream's `time_base` and the timeline's `frame_rate` rational.
- **FR-051**: Frame retrieval MUST: (1) check LRU cache first; (2) if the requested frame is behind `next_source_frame`, attempt a keyframe seek; if seek fails (non-seekable), reopen the source from the start; (3) decode forward to the target frame; (4) if the exact frame is not in cache after decoding, return the nearest prior cached frame.
- **FR-052**: When source fps < timeline fps, gap-filling MUST cache the same `FrameImage` for all gap timeline frames, preventing redundant seeks on sequential playback.
- **FR-053**: The LRU frame cache MUST have configurable capacity via `LUMEN_STREAM_CACHE_FRAMES` (default: 64). Buffer recycling MUST attempt `Arc::try_unwrap` on evicted frames; if the unwrap succeeds and capacity matches, the `Vec<u8>` is pushed to the buffer pool.
- **FR-054**: Hardware decode MUST be attempted when `LUMEN_LIBAV_HW_DEVICE` is set. On failure, the decoder MUST fall back to software decode. Platform device candidates: macOS = `videotoolbox`; Linux = `vaapi`, `cuda`, `qsv`, `vulkan`; Windows = `d3d11va`, `dxva2`, `qsv`.
- **FR-055**: FFmpeg MUST be initialized exactly once per process via `OnceLock`. Subsequent decoder constructions MUST reuse the initialized state.

#### Threading

- **FR-056**: `VideoDecodeWorker` MUST spawn one dedicated thread per video source during scene asset preparation, before the render loop begins. Workers MUST be shut down by dropping the request sender and joining the thread handle on `Drop`.
- **FR-057**: After serving a frame request, workers MUST detect the access pattern: forward sequential (current = last + 1), reverse sequential (current = last − 1), or random. Forward sequential triggers pre-decoding of frames +1 through +`LUMEN_LIBAV_PREFETCH_FRAMES` (default: 4). Reverse sequential pre-decodes frames −1 through −`LUMEN_LIBAV_PREFETCH_FRAMES`.
- **FR-058**: The render thread MUST communicate with workers via `mpsc::SyncSender<DecodeRequest>` / `mpsc::Receiver<DecodeRequest>` with bounded capacity `LUMEN_LIBAV_PREFETCH_QUEUE` (default: 8). This provides natural backpressure.
- **FR-059**: `FrameImage` MUST store pixel data as `Arc<Vec<u8>>` so it is `Send + Clone` with cheap cloning across the worker-render thread boundary.
- **FR-060**: `LibavStreamDecoder` MUST be `!Send` by default (FFmpeg C types contain raw pointers). A targeted `unsafe impl Send` MUST be applied with a documented invariant: the decoder is only ever accessed from its owning worker thread and is moved to that thread at spawn time.

#### Shadows

- **FR-061**: `BaseStyle` MUST change `shadow: Option<ShadowStyle>` to `shadows: Vec<ShadowStyle>`. An empty `Vec` means no shadows. Shadows render in declaration order (index 0 = bottommost).
- **FR-062**: Shadow blur MUST be implemented via a true Gaussian blur filter with `sigma = blur_radius / 2.0`. The current opacity-division approximation MUST be removed.
- **FR-063**: `ShadowStyle` MUST add `spread: StyleProperty<f32>` (positive = expand shadow shape, negative = contract) and `inset: bool`.
- **FR-064**: Inset shadows MUST be rendered by: clipping to the element's shape, drawing the inverted shape (large rect minus element shape) with the Gaussian blur and offset, so the result is visible only inside the element's bounds.

#### Scene and Layer Model

- **FR-065**: `Scene` MUST contain `width: u32`, `height: u32`, `frame_rate: Rational`, `duration_frames: u32`, `layers: Vec<Layer>`. Rendering a frame number outside `0..duration_frames` MUST return an error.
- **FR-066**: `Layer` MUST contain `id: String`, `clips: Vec<ClipType>`, `blend_mode: BlendMode`, `opacity: StyleProperty<f32>`, `visible: bool`. When `visible` is false, the layer MUST contribute no pixels to the output.
- **FR-067**: Layers MUST be composited bottom-to-top as units: render all clips in the layer to a layer surface, then composite the layer surface onto the scene canvas using the layer's `blend_mode` and resolved `opacity`.

#### Error Propagation

- **FR-068**: All shape draw functions MUST return `Result<(), RenderError>`. No shape draw helper may silently swallow failures.
- **FR-069**: `RenderError` MUST include clip ID and frame number in all error variants where the failing clip is identifiable.
- **FR-070**: Expression resolution failures during style resolution MUST NOT panic. They MUST return `None` (causing `resolve_or` to use the fallback) and log a warning in debug builds.

### Key Entities

- **Scene**: Root. `width: u32`, `height: u32`, `frame_rate: Rational`, `duration_frames: u32`, `layers: Vec<Layer>`. Entry point for all rendering.
- **Layer**: `id: String`, `clips: Vec<ClipType>`, `blend_mode: BlendMode`, `opacity: StyleProperty<f32>`, `visible: bool`. Composited as a unit.
- **ClipType**: `enum { Group(GroupClip), Layout(LayoutClip), Image(ImageClip), Video(VideoClip), Shape(ShapeClip), Text(TextClip) }`. Zero-cost enum dispatch via exhaustive `match`.
- **ClipMeta**: `id: Option<String>`, `start_frame: u32`, `end_frame: u32`. Shared identity and timing for all clips.
- **ClipGeometry**: `x, y, width, height, anchor_x, anchor_y` — each `StyleProperty<f32>`. Anchor in [0.0, 1.0] fractional bounds.
- **StyleProperty\<T\>**: `enum { Value(StyleValue<T>), Sequence(Sequence<T>) }`. `StyleValue<T>`: `Literal(T)` or `Expression(StyleExpression<T>)`. `Sequence<T>`: `Vec<Keyframe<T>>`.
- **Keyframe\<T\>**: `frame: u32`, `value: StyleValue<T>`, `easing: Easing`.
- **Easing**: `Linear`, `EaseIn`, `EaseOut`, `EaseInOut`, `CubicBezier(f32, f32, f32, f32)`, `Steps(u32, StepPosition)`.
- **StyleContext**: `frame: u32`, `scope: Option<&ExpressionScope>`. Passed by reference to every property resolution call.
- **BaseStyle**: `transform: TransformStyle`, `opacity: StyleProperty<f32>`, `shadows: Vec<ShadowStyle>`, `mask: Option<Mask>`, `clip_radius: [StyleProperty<f32>; 4]`.
- **TransformStyle**: `translate: [StyleProperty<f32>; 2]`, `scale: [StyleProperty<f32>; 2]`, `rotation: StyleProperty<f32>`, `skew: [StyleProperty<f32>; 2]`, `origin: [StyleProperty<f32>; 2]`.
- **Fill**: `Solid { color: [StyleProperty<u8>; 4] }` | `LinearGradient { start, end, stops }` | `RadialGradient { center, radius, stops }` | `Image { source, fit }`.
- **GradientStop**: `position: StyleProperty<f32>`, `color: [StyleProperty<u8>; 4]`.
- **Stroke**: `color: [StyleProperty<u8>; 4]`, `width: StyleProperty<f32>`, `dash_pattern: Option<Vec<f32>>`, `line_cap: LineCap`, `line_join: LineJoin`.
- **TextStyle**: `font_family: String`, `font_size: StyleProperty<f32>`, `font_weight: StyleProperty<u32>`, `font_style: FontSlant`, `color: [StyleProperty<u8>; 4]`, `line_height: StyleProperty<f32>`, `letter_spacing: StyleProperty<f32>`, `text_align: TextAlign`, `vertical_align: VerticalAlign`, `max_width: Option<StyleProperty<f32>>`, `max_lines: Option<u32>`, `overflow: TextOverflow`, `decoration: TextDecoration`.
- **ShadowStyle**: `offset_x: StyleProperty<f32>`, `offset_y: StyleProperty<f32>`, `blur: StyleProperty<f32>`, `spread: StyleProperty<f32>`, `color: [StyleProperty<u8>; 4]`, `inset: bool`.
- **Mask**: `source: MaskSource`, `inverted: bool`. `MaskSource`: `Shape(MaskShape)` | `Bitmap { source: String }` | `Clip { clip_id: String }`. `MaskShape`: `Rectangle { x, y, width, height, corner_radius: [StyleProperty<f32>; 4] }` | `Ellipse { cx, cy, rx, ry }` | `Path { data: Vec<PathCommand> }`.
- **Expression**: Parsed AST + `references: Vec<ExpressionReference>` + original source string. Created via `Expression::parse`.
- **ExprNode**: `Number(f32)` | `Boolean(bool)` | `String(String)` | `ClipRef { clip_id, property }` | `LayoutRef { node_id, property }` | `BinaryOp { op, left, right }` | `UnaryOp { op, expr }` | `FuncCall { name, args }` | `Conditional { condition, then, otherwise }`.
- **ExpressionScope**: `clip_properties: HashMap<(String, ExpressionProperty), ExpressionValue>`, `layout_properties: HashMap<(String, ExpressionProperty), ExpressionValue>`.
- **ExpressionValue**: `Number(f32)` | `Boolean(bool)` | `String(String)`.
- **ExpressionProperty**: `X`, `Y`, `Width`, `Height`, `Opacity` (minimum set; extensible).
- **DependencyPlan**: `evaluation_order: Vec<DependencyNode>` sorted by Kahn's algorithm.
- **DependencyNode**: `ClipRender { clip_id: String, layer: usize }` | `LayoutCompute { layout_id: String }` | `Expression(ExpressionId)`.
- **ResultMap**: `HashMap<(String, ExpressionProperty), ExpressionValue>`. Populated incrementally during the ordered render pass.
- **LayoutNode**: `id: Option<String>`, `style: taffy::Style`, `content: Option<LayoutContent>`, `children: Vec<LayoutNode>`.
- **LayoutContent**: `Text(TextClip)` | `Image(ImageClip)` | `Video(VideoClip)` | `Shape(ShapeClip)` | `Layout(LayoutClip)`.
- **ImageFit**: `Cover` | `Contain` | `Fill` | `None`.
- **LoopMode**: `None` (hold last frame) | `Repeat` (modulo) | `PingPong` (reverse at boundary).
- **LibavStreamDecoder** (feature = "ffmpeg"): `input_ctx`, `decoder`, `scaler` (swscale RGBA converter), `width/height`, `time_base`, `timeline_time_base`, `next_source_frame`, `cache: LruCache<u64, FrameImage>`, `buffer_pool: Vec<Vec<u8>>`, `eof`, `draining`.
- **VideoDecodeWorker** (feature = "ffmpeg"): `source_id: String`, `tx: Option<mpsc::SyncSender<DecodeRequest>>`, `handle: Option<thread::JoinHandle<()>>`. Drop shuts down the thread.
- **FrameImage**: `Arc<Vec<u8>>` RGBA pixel buffer + `width: u32` + `height: u32`. `Send + Clone`.
- **ProvidedFrame**: `Ready(FrameImage)` | `EndOfStream` | `Missing`.

---

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A rendered frame containing 10 overlapping styled shapes (each with fill, stroke, transform, and at least one shadow) completes in under 16 ms on the software CPU backend at 1080p, targeting 60 fps headroom.
- **SC-002**: Keyframe interpolation for any easing variant returns a value within ±0.001 of the mathematically correct result as computed by the CSS cubic-bezier reference algorithm.
- **SC-003**: Text rendering for a 500-character paragraph with line wrapping and a `max_width` constraint completes in under 5 ms per frame.
- **SC-004**: A scene with 3 simultaneously active 1080p video clips renders at sustained 24 fps without frame drops when hardware-accelerated decoding is available.
- **SC-005**: Dependency graph construction and topological sort for a scene with 50 clips and 20 cross-clip expression references completes in under 1 ms.
- **SC-006**: Seeking to a random frame in a video source (cold LRU cache) returns the correct decoded RGBA frame within 200 ms.
- **SC-007**: Forward sequential video playback achieves a per-source LRU cache hit rate above 95 %, measured over 100 consecutive frame requests, due to lookahead prefetch.
- **SC-008**: 100 % of clip types (Group, Layout, Image, Video, Shape, Text) produce real visual output in all production-mode renders. Zero debug-placeholder pixels are emitted by any clip.
- **SC-009**: The expression evaluator returns a typed `ExpressionError` (not a panic) for every category of failure: parse error, unknown function, type mismatch, unresolved reference.
- **SC-010**: All rendering operations on all code paths return `Result<_, RenderError>`. No `unwrap()` or `expect()` calls exist in any file under `src/` within the `lumen` crate's render path.
- **SC-011**: The full Kahn topological sort produces a correct ordering (verified by property-based tests with randomly generated dependency graphs containing no cycles) in under 0.1 ms for graphs with up to 200 nodes.
- **SC-012**: Reverse sequential video playback (PingPong) achieves a per-source LRU cache hit rate above 80 % when the prefetch direction has switched to reverse, measured over 30 consecutive frames around the reversal point.
