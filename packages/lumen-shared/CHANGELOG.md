# @lumiscia/lumen-shared

## 0.3.1

### Patch Changes

- @lumiscia/lumen-types@0.3.1

## 0.3.0

### Minor Changes

- 3f45d73: The GPU renderer now renders unchanging parts of a composition once and reuses them: a node whose params hold no expression and that reads no media or shader time, fed only by such nodes, is neither bound nor dispatched again on later frames of its plan. Output frames are unchanged. Kept output lives in the plan's own textures, so it counts against the plan cache budget and is dropped with its plan, and recompiles, font loads and texture replacements render it again.

  `memo` now caches. Its `cache_id` and `allow_expressions` params are removed in favour of `key`, an expression evaluated at the frame being rendered: while it gives the same value as on the frame `memo` last rendered, `memo` shows that output and nothing only it reads is bound or dispatched. `"key": "=floor(frame / 4)"` runs a heavy shader on one frame in four, and `"=floor(frame / 2)"` animates on twos. What `memo` holds is the source as rendered on the first frame of each key value that is rendered, so pair it with a `time_remap` to `floor(frame / 4) * 4` when frames may render out of order. The snapshot belongs to the memo node and its time-remap path rather than to a plan, so fractional frames and upstream switches that bind another plan show it too; it is taken again once every plan holding it is evicted, and any edit to the composition drops it, since a key holds output across frames, never across edits. Bounds a `crop` publishes are kept with the pixels. A literal or missing `key` adds nothing to the caching above. From TypeScript, `composition.addNode({ type: "memo", params: { key: floor(frame.div(4)) } })`.

  `GpuCompositionRenderer::cached_pass_count` and `submitted_pass_count` report how many passes were skipped and submitted, and `lumen-local` prints the per-frame averages as `static_cached` and `passes`.

- 076bf72: Text nodes take `glyph_offset`, `glyph_scale`, `glyph_rotation` and `glyph_opacity`, evaluated for every glyph to stagger glyphs, words and lines apart. Inside them, expressions can read the `glyph`, `word` and `line` index of the glyph being drawn and the `glyph_count`, `word_count` and `line_count` of the text, such as `"glyph_opacity": "=tween(0, 1, 10 + glyph * 2, 12)"`; elsewhere those globals are an error. Each glyph scales and turns about the middle of its advance and line, before the text's own transform, and fades before its `opacity`. `@lumiscia/lumen-shared` exports them as `glyph`, `word`, `line`, `glyphCount`, `wordCount` and `lineCount`.
- b0bbf52: Default new compositions to 30 fps in the TypeScript builder, matching the engine, and convert audio `start_frame`/`duration_frames` at the composition's frame rate instead of a fixed 30. Compositions that relied on the old 24 fps builder default now play faster (240 frames last 8 s instead of 10 s); call `setFps(24)` to keep the old timing.
- 1ab946d: Add typed expression helpers that build `=...` expression strings: one helper per expression builtin (`tween`, `vec2`, `rgb`, `spring`, `outExpo`, ...), `ease.*` easing names, `sec`/`ms` units, `hex` colours, the `frame`/`time`/`fps`/`width`/`height` globals, and `.add`/`.sub`/`.mul`/`.div`/`.oneMinus`/`.clamp` with the evaluator's vec2 and colour rules. `fn("rise(start)", ...)` defines a composition function, which `Composition.addFunction` adds to `functions`, and `moves(base, [...])` sums tweens. Node params accept these expressions directly.
- 4e61e7a: `wgsl_shader` now takes up to 16 inputs (`input0..input15` plus a shared `input_sampler`), gives every shader `lumen.time`, `lumen.frame` and `lumen.resolution`, binds typed uniforms by name from the shader's uniform struct, rebuilds its pipeline when the source changes, premultiplies straight-alpha inputs before the shader sees them, and rejects unknown or malformed uniform values and shader bindings above 2 instead of ignoring them.

  Uniforms are set with a `uniforms` object instead of the `bindings` string, which is removed. Each value is a number, an array of 2-4 numbers or `=expression`s, a whole-value `=expression`, or, for `vec3<f32>` and `vec4<f32>` fields, a `#rrggbb` / `#rrggbbaa` colour or a colour expression such as `=rgb(255, 128, 0)`. `wgslShader()` values also take typed `Expr`s from the expression helpers. The schema and the generated `ShaderUniforms` / `ShaderUniformValue` types describe it, and `wgslShader()` in `@lumiscia/lumen-shared` declares the uniform struct in TypeScript and type-checks each field's value.

  The GPU renderer now keeps compiled plans within an estimated memory budget (1 GiB by default, configurable with `GpuCompositionRenderer::set_plan_cache_budget`), dropping least recently used plans instead of keeping one per animated switch or shader combination.

### Patch Changes

- 38a05a8: Prevent explicit and automatically allocated composition node IDs from colliding.
- Updated dependencies [2c68ade]
- Updated dependencies [5726783]
- Updated dependencies [577d694]
- Updated dependencies [d52b7d6]
- Updated dependencies [b314e00]
- Updated dependencies [3f45d73]
- Updated dependencies [076bf72]
- Updated dependencies [2b50b73]
- Updated dependencies [f054aaf]
- Updated dependencies [97f55bf]
- Updated dependencies [1b30cb7]
- Updated dependencies [d658d6c]
- Updated dependencies [b88aeda]
- Updated dependencies [4e61e7a]
  - @lumiscia/lumen-types@0.3.0

## 0.2.2

### Patch Changes

- @lumiscia/lumen-types@0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.
- cb6c9fa: Add composition audio track and clip builder helpers with canonical timeline output.

### Patch Changes

- Updated dependencies [072169a]
- Updated dependencies [cb6f4dc]
  - @lumiscia/lumen-types@0.2.0

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.

### Patch Changes

- Updated dependencies [906481a]
  - @lumiscia/lumen-types@0.1.0
