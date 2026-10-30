# @lumiscia/lumen-bindings

## 0.3.1

### Patch Changes

- ea6e7da: Editing a node's params in place and rendering again with the same renderer now draws the edit instead of reusing the plan compiled before it. Any change to the composition, whether a literal param, an expression source, a connection, the canvas or a template, compiles a fresh plan and drops the cached ones.
- Updated dependencies [c44cca1]
- Updated dependencies [b506afc]
  - @lumiscia/lumen-preview@0.3.1

## 0.3.0

### Minor Changes

- a48252c: Shape, path, text and `transform` nodes take a `motion_blur` shutter angle in degrees, which smears the layer along its motion over a shutter centred on the frame. Expressions can now be evaluated between frames, so `frame` and `time` may be fractional during the shutter.
- 20ce332: `crop` now keeps its region in place and `resize` gains `x`/`y` placement, and both accept animated sizes instead of fixing them at compile time.
- 3b3147f: Shape, path and text nodes take their own `scale`, `rotation`, `anchor` and `opacity`, applied analytically so scaled vector edges stay sharp, and paths and polygons can have up to 16384 points instead of 128.
- 96edee6: Add `--bitrate`, `--crf`/`--qp` and `--preset` encoder quality flags to `lumen-local`, backed by new `rate_control` and `preset` fields on `VideoEncoderConfig`.
- 577d694: Add expression animation builtins: `progress`, `tween`, the `in`/`out`/`in_out` quad, cubic, expo and back easings, `cubic_bezier` and `spring`, plus `s` and `ms` suffixes on numbers. `vec2` values now support `+`, `-` and scaling, and `lerp`/`tween` work on numbers, `vec2` values and colours. Colours are expression values (`#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`), so colour and paint params accept expressions. Compositions can declare reusable `functions`, such as `"rise(start)": "tween(vec2(0, 60), vec2(0, 0), start, 16, 'out_expo')"`, which expand where they're called. The builtin table, easing names and globals are published in the schema and as `expressionBuiltins`, `easingNames`/`EasingName` and `expressionGlobals` in the types.
- 00b258d: Expressions now see fractional `frame` and `time` values, and `time_remap` keeps fractional target frames instead of flooring them.
- 6360031: Replace the box blur in `blur` and `shadow` with a shared separable Gaussian that supports fractional and large radii.
- ef68eec: Text at large sizes now draws glyphs bigger than the atlas, renders MSDF with sharp corners at 1000 px and above by raising the field resolution with size, and adds an `auto` render mode that switches to MSDF from 512 px.
- 3f45d73: The GPU renderer now renders unchanging parts of a composition once and reuses them: a node whose params hold no expression and that reads no media or shader time, fed only by such nodes, is neither bound nor dispatched again on later frames of its plan. Output frames are unchanged. Kept output lives in the plan's own textures, so it counts against the plan cache budget and is dropped with its plan, and recompiles, font loads and texture replacements render it again.

  `memo` now caches. Its `cache_id` and `allow_expressions` params are removed in favour of `key`, an expression evaluated at the frame being rendered: while it gives the same value as on the frame `memo` last rendered, `memo` shows that output and nothing only it reads is bound or dispatched. `"key": "=floor(frame / 4)"` runs a heavy shader on one frame in four, and `"=floor(frame / 2)"` animates on twos. What `memo` holds is the source as rendered on the first frame of each key value that is rendered, so pair it with a `time_remap` to `floor(frame / 4) * 4` when frames may render out of order. The snapshot belongs to the memo node and its time-remap path rather than to a plan, so fractional frames and upstream switches that bind another plan show it too; it is taken again once every plan holding it is evicted, and any edit to the composition drops it, since a key holds output across frames, never across edits. Bounds a `crop` publishes are kept with the pixels. A literal or missing `key` adds nothing to the caching above. From TypeScript, `composition.addNode({ type: "memo", params: { key: floor(frame.div(4)) } })`.

  `GpuCompositionRenderer::cached_pass_count` and `submitted_pass_count` report how many passes were skipped and submitted, and `lumen-local` prints the per-frame averages as `static_cached` and `passes`.

- 137382c: Merge gains `mask_mode` (alpha or luma), `invert` and `feather` mask options, the boolean node keeps antialiased edges at its default threshold, and shapes can now be 0 pixels wide or tall to draw nothing.
- 076bf72: Text nodes take `glyph_offset`, `glyph_scale`, `glyph_rotation` and `glyph_opacity`, evaluated for every glyph to stagger glyphs, words and lines apart. Inside them, expressions can read the `glyph`, `word` and `line` index of the glyph being drawn and the `glyph_count`, `word_count` and `line_count` of the text, such as `"glyph_opacity": "=tween(0, 1, 10 + glyph * 2, 12)"`; elsewhere those globals are an error. Each glyph scales and turns about the middle of its advance and line, before the text's own transform, and fades before its `opacity`. `@lumiscia/lumen-shared` exports them as `glyph`, `word`, `line`, `glyphCount`, `wordCount` and `lineCount`.
- f6e2ddb: Add `letter_spacing` and `line_height` to the text node and let `font_weight` take any value from 1 to 1000 instead of snapping to hundreds.
- 1b55b88: On Linux, `lumen-local --encoder av1_vulkan` (or `h264_vulkan`/`hevc_vulkan`) now renders and encodes on one shared Vulkan device, converting frames to NV12 on the GPU instead of reading them back. It falls back to the CPU upload path when the device has no Vulkan Video encode support or separate compute queue family, or when the frame size is odd. The new `VulkanDevice`, `VulkanRenderQueue` and `VideoEncoderConfig::vulkan_device` back `EncodeMode::GpuTexture(GpuBackend::Vulkan)`.
- 4e61e7a: `wgsl_shader` now takes up to 16 inputs (`input0..input15` plus a shared `input_sampler`), gives every shader `lumen.time`, `lumen.frame` and `lumen.resolution`, binds typed uniforms by name from the shader's uniform struct, rebuilds its pipeline when the source changes, premultiplies straight-alpha inputs before the shader sees them, and rejects unknown or malformed uniform values and shader bindings above 2 instead of ignoring them.

  Uniforms are set with a `uniforms` object instead of the `bindings` string, which is removed. Each value is a number, an array of 2-4 numbers or `=expression`s, a whole-value `=expression`, or, for `vec3<f32>` and `vec4<f32>` fields, a `#rrggbb` / `#rrggbbaa` colour or a colour expression such as `=rgb(255, 128, 0)`. `wgslShader()` values also take typed `Expr`s from the expression helpers. The schema and the generated `ShaderUniforms` / `ShaderUniformValue` types describe it, and `wgslShader()` in `@lumiscia/lumen-shared` declares the uniform struct in TypeScript and type-checks each field's value.

  The GPU renderer now keeps compiled plans within an estimated memory budget (1 GiB by default, configurable with `GpuCompositionRenderer::set_plan_cache_budget`), dropping least recently used plans instead of keeping one per animated switch or shader combination.

### Patch Changes

- 36c4af9: Tag encoded video as BT.709 with matching color conversion, encode fractional frame rates such as 29.97 exactly, and rebuild the RGBA converter when decoded frame geometry changes.
- 86eda9e: Shapes, paths, solid-colour text and transformed layers render into textures sized to the pixels they cover rather than the whole canvas, and merges, multimerges, opacity and colour filters work within those bounds, cutting GPU memory and frame time for compositions with many small elements. Output frames are unchanged.
- 2cf81aa: Parse and render errors now name the node and property involved, and colour expressions are rejected when the composition is parsed instead of when it renders.
- cf195bd: Fractional values for integer params now report "must be a whole number", and compositions still using the removed `bindings` param on `wgsl_shader` are pointed to `uniforms`.
- d52b7d6: Allow `=` expressions in the schema and TypeScript types exactly where the engine accepts them. Paints, colours, named enums and `alpha_premultiply.mode` no longer type-check with an expression.
- 15a1db6: Reject lossy audio volume and integer values with errors that name the field, report which part of an invalid color was wrong, and drop the unused `x-lumen-nodeKinds` schema key. Native GPU instances now discard HAL debug labels by default to avoid a Vulkan loader crash with concurrent devices; set `WGPU_DISCARD_HAL_LABELS=0` to keep resource names in GPU captures.
- 17d63ec: Allow string params to start with a literal `=` by escaping it as `==`: `"==wow"` renders `=wow`, while `"=frame"` is still an expression.
- e40a08b: Reject invalid timeline fps, missing or invalid duration frames, malformed background colors, out-of-range color components, malformed gradient stops and unparsable WGSL shader bindings with clear errors instead of silently falling back to defaults.
- 0a08ad1: Render inputs connected from node 0 instead of treating them as unconnected, which left the first node of every SDK-built composition transparent.
- d8e548e: Sample Transform and Resize at pixel centres so identity transforms with linear sampling reproduce the input exactly, and add a Transform `custom_pivot` option so a (0, 0) pivot can mean the top-left corner instead of the source centre.
- d39faea: Render `shape` polygons from `polygon_points` instead of drawing a rectangle, and draw nothing while the point list is empty or incomplete.
- 6bc1c81: Treat rasters as premultiplied alpha consistently. Merges no longer darken translucent overlays, shape and path strokes composite over fills correctly, gradients interpolate in premultiplied space, and colour adjustments keep fully transparent pixels transparent. Compositing nodes and the default WGSL shader now honour straight-alpha inputs, and the Alpha Premultiply mode must be a constant rather than an expression.
- 341b369: Accept a single number for a gradient `radius`, as the schema allows, and name the failing field when a paint fails to parse instead of a generic "expected paint" error.
- 2b50b73: Add opt-in `render_mode: "msdf"` text rendering through the `msdf` Cargo feature (enabled by default). Raster and MSDF glyphs share a fixed 4096² RGBA8 atlas costing 64 MiB per renderer once text is used. Standalone GPU callers can install the atlas with `CompiledComposition::prepare_text_atlas`.
- 3712329: Draw raster text on whole pixels so it stays sharp at fractional positions, keep glyph memory bounded when font sizes animate, warn instead of silently dropping glyphs when the atlas is full, measure relative text gradients against the text's own bounds, render colour emoji without dark fringes, and make `textWidth` use the same fonts as text rendering.
- e030363: Keep preview commands applied while a render is in flight, reuse the GPU device across composition and media changes, and bound the wasm video frame cache by bytes through `frameCacheCapacity`.
- b0bbf52: Default new compositions to 30 fps in the TypeScript builder, matching the engine, and convert audio `start_frame`/`duration_frames` at the composition's frame rate instead of a fixed 30. Compositions that relied on the old 24 fps builder default now play faster (240 frames last 8 s instead of 10 s); call `setFps(24)` to keep the old timing.
- 0aaa4a1: Clamp negative shape border radii to zero so expressions cannot distort rectangles.
- 9c0b0b7: Bind each time-remapped branch with its own frame parameters when a node is reached through more than one time context.
- fcb8095: Update the Rust engine dependencies, including wgpu 30 and cosmic-text 0.19.
- Updated dependencies [482ec57]
- Updated dependencies [e030363]
  - @lumiscia/lumen-preview@0.3.0

## 0.2.2

### Patch Changes

- 9d1fc11: Fix gradient paint rendering for conic angle offsets and text fills.
  - @lumiscia/lumen-preview@0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.

### Patch Changes

- Updated dependencies [072169a]
- Updated dependencies [cb6f4dc]
  - @lumiscia/lumen-preview@0.2.0

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.
