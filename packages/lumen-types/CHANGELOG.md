# @lumiscia/lumen-types

## 0.4.0

### Minor Changes

- c1c57b2: `addNode` takes `inputs`, so a node connects to the nodes that feed it where it is added: `addNode({ type: "merge", inputs: { base: background, overlay: title } })`. A variadic port such as `layers` takes an array, `{ layers: [background, image, title] }`. Ports are typed from the node kind, and the new generated `NodeVariadicInputPortName<Kind>` marks the ones that take arrays. The connections are the ordinary ones, in the order written, so the JSON is the same as with `connect`, which stays. `literal(text)` escapes a leading `=` as `==`, and `expr("if(frame > 30, 1, 0)")` wraps raw expression text, such as `if(..)` and `node(..)`, as an `Expr`.

  Breaking: `Composition` loses `setSize`, `setFps`, `setDurationFrames`, `setDurationSeconds`, `setMetadata` and `setAudio`; pass `renderSettings`, `timeline` and `metadata` to the constructor. `connect` loses `fromPort`, which the engine only accepts as `output`. `connect` and `inputs` throw for a node handle from another composition; numeric ids are left to the engine. A failed `addNode` leaves the composition, including its templates, functions and next automatic id, as it was. A composition definition no longer has `.composition`. `moves(base, [{ by, .. }])` emits `by * tween(0, 1, ..)` instead of `tween(0, by, ..)`, with the same value, and `Expr` no longer tracks a kind. `fn` and `addFunction` no longer check names and `wgslShader` no longer checks uniform names or types; the engine rejects function names when the composition loads, and the shader compiler reports bad uniform names and types. A `toJSON` on a plain array in a param, or a non-enumerable one on a plain param object, is no longer called: `{ text: "draft" }` with a hidden `toJSON` returning `{ text: "final" }` now serializes as `draft`. An enumerable `toJSON` on a plain object still runs, and so does one on a class instance or array subclass.

- b6ece6b: Composition errors now name the JSON path, such as `timeline.fps: ...`, and more mistakes fail instead of being ignored. Unknown keys in `timeline` and `render_settings` are errors, including in templates, so a typo like `fpss` no longer falls back to 30 fps. Other unknown keys at the root and in `audio` are still ignored.

  `render_settings.background_color`, which rendering never read, is an error: connect a `background` node instead. `Composition.setBackgroundColor` is gone. Audio clips need whole-number `start_ms` and `duration_ms`, and `source_start_ms` must be a whole number when set; `start_frame`, `duration_frames` and `source_start_seconds` are errors. `schemaVersion` is ignored and the schema no longer types it, `lumenSchemaVersion` must be absent or `0.1.0` in the schema too, and a non-string `$schema` or a `metadata` that isn't an object with a string `name` now fails at load, as it already did in the schema. The builder no longer writes `durationSeconds` into `timeline`. `CompositionBuilder::build` now rejects fps that isn't positive and finite, a zero size or duration, and a prop default of the wrong kind, for the root and templates.

- 6b3a821: `Composition.connect()` takes the target port explicitly instead of defaulting to `input`, which no node has, so a connection without `toPort` no longer builds a composition the engine rejects. Given the target node, `toPort` is typed to its inputs, from the new generated `NodeInputPortName<Kind>` type: `composition.connect(background, merge, { toPort: "base" })` type-checks, and `toPort: "source"` on a `merge` does not. `addNode` now infers the node kind it returns.
- 0003560: Expressions fail earlier and accept more number forms. An unknown variable such as `=frmae + 1` is now rejected when the composition loads instead of failing mid-render, an unknown string escape such as `'\q'` is an error rather than dropping the backslash, and `node(id)` rejects ids above 2^53. Numbers may now use exponents and a leading dot (`1e3`, `1.5e-3`, `.5`), and a number too large to represent is rejected rather than becoming infinity. The SDK's helpers write large and small numbers in exponent form as a result.

  The `linear(a, b, t)` builtin and the SDK's `linear` helper are gone; use `lerp`. `'linear'` stays as an easing name. `x-lumen-expressions` in the schema now lists `forms` (`if` and `node`) and `reserved` names, which `@lumiscia/lumen-types` exports as `expressionForms` and `expressionReservedNames`.

- 75572c3: The composition schema's paint, shader uniform and clip prop `$defs` are now generated from the Rust param definitions, with the same accepted values as before. New `$defs` name the shapes that were inline: `gradientPaint`, `gradientStop`, `paintKind`, `gradientUnits`, `gradientSpread`, `gradientInterpolation`, `shaderUniformComponent`, `shaderUniforms` and `clipProps`; gradient fields carry descriptions, defaults and enum options. `@lumiscia/lumen-types` generates `Paint`, `GradientPaint`, `GradientStop`, `ShaderUniforms`, `ShaderUniformValue`, `ShaderUniformComponent`, `ClipProps` and `ClipPropValue` from these `$defs`, with JSDoc, and exports the gradient enums as `PaintKind`, `GradientUnits`, `GradientSpread` and `GradientInterpolation`.

  For Rust: `GradientPaintDelegate` and `GradientStopDelegate` are exported from `vector::paint`, and the gradient enums implement `NodeEnum` with an `enum` property kind.

  Required fields and enum aliases come from the Rust types: `#[meta(required)]` on a `Delegate` field marks it required in the schema and, for `#[delegate(properties)]` structs, makes deserializing reject objects without it. `PropertyDef` gains `required` and `EnumOptionDef` gains `aliases`, read from the variant's `#[serde(alias)]` attributes.

- eb354e6: The text node draws underlines, strikethroughs and highlights. The new node-level `underline`, `strikethrough` and `highlight` params are off when absent and apply to string content and to every span that leaves its own unset. A span's own decoration replaces the node's, and `null` turns it off for that span.

  Consecutive spans that take the node's decoration form one run, which draws one line or box per line of text, so mixed font sizes don't step or seam. A line's absent `thickness` and `offset` come from the tallest span on that line, and its absent `paint` is the text's own paint. Highlights break per line like CSS `box-decoration-break: clone`: each line gets its own padding and corner radius. `extent` maps `0..1` along the run's text in reading order, continuing across line breaks and following right-to-left text, so `"extent": "=vec2(0, clamp(time * 2, 0, 1))"` draws an underline in across wrapped lines. Lines can be `solid`, `double`, `dotted`, `dashed` or `wavy`.

  Decorations are painted in CSS order (highlights, underlines, glyphs, then strikethroughs). They follow the node's transform, opacity and motion blur but not the `glyph_*` params. They never change the text's layout or `text_width` and `text_height`, and they count toward the text node's glyph limit. Text without decorations renders as before.

  For Rust: `TextParams` has `underline`, `strikethrough` and `highlight` fields. `GpuGlyphInstance` has a `GLYPH_MODE_RECT` render mode for decoration rects, made with `GpuGlyphInstance::rect`, and `ShapedText::decorate` lays out a `TextDecoration`. Node params can now be `Option<T>`, unset unless given, and their specs then have no default value. The generated `NodeParamSpec.defaultValue` is optional.

- 4f3894f: The text node takes a `font_features` param, and spans take the same field: OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0, "ss01": 1 }`. A value of `0` turns a feature off, `1` turns it on and larger values select an alternate. A span's features merge over the node's: the span's keys win and the rest are inherited. Tags must be exactly 4 printable ASCII characters and values non-negative integers, and errors name the field. Each value takes a literal or an `=expression`, but the whole map cannot be an expression.

  Features shape the text, so `text_width`, `text_height` and `text_bounds` follow them. A word takes the features of the first span that starts it, because cosmic-text shapes the spans of a word together. Text without features renders as before.

  For Rust: `lumen_text::TextSpan` has a `font_features` field of `FontFeature` settings, and the new `FontFeatures` param kind is generated as `FontFeatures` in `@lumiscia/lumen-types`.

- 93fb454: The text node can limit its text to a number of lines or a box, and shrink text to fit. The new `max_lines` and `max_height` params drop the lines past them, and are unlimited when absent. `overflow` sets how the last line shown ends: `clip`, the default, leaves it as it is, and `ellipsis` ends it with `…`. Clusters at that line's end, and whitespace before the ellipsis, are dropped until it fits `max_width`. The ellipsis goes at the line's end in reading order, so it sits on the left of right-to-left text, and it takes the style, paint and decorations of the text it follows.

  `fit: "shrink"` scales every font size, the node's and each span's, by one factor of at most 1, so the text fits `max_width`, `max_lines` and `max_height` without breaking a word. Letter spacing, a pixel `line_height`, and decoration thickness, offset, padding and radius scale with it. The factor is the largest that fits, down to the one that takes `font_size` to `min_font_size` (8 by default); lines that still don't fit there are dropped by `overflow`. A fit takes at most 12 shapes, and is kept for the text's layout, so measuring, bounding and drawing the same text share one fit, and constant text pays for it once.

  `text_width`, `text_height`, bounded output and decorations all cover the text as drawn, after truncation and fit. The `glyph_*` params count only the glyphs and lines shown. Glyphs keep their numbers, and the ellipsis is one glyph numbered after the last. Text that sets none of these params lays out as before.

  For Rust: `TextParams` has `max_lines`, `max_height`, `overflow`, `fit` and `min_font_size` fields. `TextLayoutRequest` has `max_lines`, `max_height`, `overflow` and `min_fit_scale`, and `ShapedText::fit_scale` gives the factor a fit chose.

- 5a0939c: Add the `text_content`, `text_line` and `text_highlight` param kinds for rich text. No node uses them yet. Text content is a string or an array of spans. Each span needs `text` and can override the node's font family, size, weight, style, letter spacing and color, and can set an underline, strikethrough or highlight; `null` turns off a decoration the span would otherwise inherit. Span fields, including enums, accept `=expressions` like node params, and evaluation errors name their path, such as `content[1].font_weight`. The composition schema describes these kinds in `$defs` (`textContent`, `textSpan`, `textLine`, `textHighlight`, `textLineStyle`), which are generated from the Rust param definitions. `@lumiscia/lumen-types` exports matching `TextContent`, `TextSpan`, `TextLine`, `TextHighlight` and `TextLineStyle` types.

  For Rust: nested `Delegate` structs can now have `Option<T>` fields, which are left out when absent, and `Override<T>` fields, which inherit, clear or set a value. A struct delegate marked `#[delegate(properties)]` parses its nested JSON field by field the way node params do, rejects unknown fields, and has a `to_json` method; other struct delegates still deserialize through serde. `PropertyDef` has a new `nullable` field. `TextFontStyle` accepts expressions, which also lets the text node's `font_style` take one.

- 37500c3: The text node's `content` takes styled spans as well as a string. Each span's font family, size, weight, style, letter spacing and color override the node's, and anything a span leaves out comes from the node. Span text and every span field can be an `=expression`. String content, including `=expressions`, renders as before. Span decorations parse but are not drawn yet.

  Each span is drawn in its own `color` paint. The new `paint_space` param (`span` by default, or `text`) sets whether a span's own gradient covers just that span or the whole text. The node's `color`, which spans without a color use, always covers the whole text. `paint_supersample` still applies to the whole node.

  The `glyph_*` params can read the new `span` and `span_count` expression globals, so they can target a single span, as in `=if(span == 1, vec2(0, -10 * sin(time * 6 + glyph)), vec2(0, 0))`. `@lumiscia/lumen-shared` exports them as `span` and `spanCount`, and a text node's `content` takes a string `Expr`. `text_width(node(id))` and `text_height(node(id))` measure the spans, and `node(id, "content")` reads span content as its joined text when that text is literal.

  For Rust: `TextParams::content` is now a `TextContent`, and `TextParamsDelegate::content` is now a `TextContentDelegate`, so wrap string content in `TextContentDelegate::Text`. `GpuGlyphInstance` packs a paint index into the upper bits of `mode` (read and set with `render_mode()`, `paint()` and `set_paint()`), and `GpuTextGlobals` no longer has `paint_bounds`, since each paint now carries its own bounds.

- f35c132: The text node can outline its glyphs. The new node-level `stroke` param is off when absent and applies to string content and to every span that leaves its own unset. A span's own `stroke` replaces the node's, and `null` turns it off for that span. A stroke has a `paint`, a `width` in pixels and an `align`, and each field takes a literal or an `=expression`:

  ```json
  { "stroke": { "paint": [0, 0, 0, 255], "width": "=4 + 2 * sin(time * 6)", "align": "outside" } }
  ```

  `outside`, the default, draws the outline behind every glyph's fill, reaching `width` pixels past the glyph's edge, as video captions do. The fill covers the outline's inner half. `center` draws the outline over the fills, centred on the glyph's edge, like `-webkit-text-stroke`. Joins are round. The stroke's paint respects `paint_space` the way a span's `color` does. Strokes follow their glyph's `glyph_*` transform and fade, and the node's transform, opacity and motion blur. Decorations are not stroked.

  Distance-field glyphs draw their stroke from a true distance field in the text shader, at the fill's resolution, whose distance range doubles as often as the stroke needs, so animated widths stay smooth and share fields. Raster glyphs draw a stroked outline, keyed by its half width rounded to a quarter pixel up to 4 px and otherwise to a sixteenth of its power of two, so animated widths reuse them. A raster stroke too large for the glyph atlas is drawn from a distance field instead, and a stroke whose field is too large for the atlas is dropped. In `auto` mode each glyph's stroke takes the path its glyph does. `glyph_opacity` fades a glyph's stroke and fill as one, so a faded glyph's fill still hides the stroke under it. Neighbouring glyphs fade separately, so where one's stroke overlaps another both show. Shrink-to-fit scales stroke widths with the text. Strokes count toward the text node's glyph limit, and `text_bounds` and motion blur cover them. They don't change the layout or `text_width` and `text_height`. Colour and bitmap glyphs are not stroked. Text without a stroke renders as before.

  For Rust: `TextParams` has a `stroke` field and `TextSpan` a `stroke` override, both of the new `TextStroke` kind. `TextSystem::prepare_stroked_glyphs` takes a half width per span and returns the stroke instances in `PreparedGlyphs::strokes`, each outlining the glyph instance `PreparedGlyphs::stroke_glyphs` names. `GpuGlyphInstance::color` is replaced by `opacity`, since the shader only read its alpha, and a `partner` index used by the new `GLYPH_PARTNER` flag.

- 0d47151: The text node gains `text_transform`, `paragraph_spacing` and, on spans, `baseline_shift`.

  `text_transform` is `none`, `uppercase`, `lowercase` or `capitalize`, on the node and on spans, where an unset span takes the node's. It maps the text before shaping with Unicode case mapping, so `ß` becomes `SS`. `capitalize` uppercases the first letter or digit of each word, and a word continues across spans. Glyph, word and line numbering, `span` indices, `text_width` and `text_height` count the transformed text, as do decoration extents, which are fractions of its bytes.

  `baseline_shift` is a span's offset in pixels, positive up. It moves the span's glyphs, its own underline, strikethrough and highlight, and its gradient box, while the node's decorations that the span inherits stay on the line. The line keeps its height, so shifted text can overlap its neighbours, unlike CSS `vertical-align` with a length. `text_bounds` includes the shift, and `text_width` and `text_height` do not. Superscripts are a smaller `font_size` with a `baseline_shift`.

  `paragraph_spacing` adds pixels after each paragraph, which a hard line break starts, but not after the last. Wrapped lines of one paragraph keep their line pitch. It adds to `text_height`.

  Text that sets none of these renders as before.

  For Rust: `lumen_text::TextSpan` has `transform` and `baseline_shift`, `TextLayoutRequest` has `paragraph_spacing`, `TextSegment` has `shift`, and `TextDecoration` has `shifted`. `ShapedText::bounds` can start above the first line when a span is raised. The generated type for the `text_transform` param is `TextTransform`, and a node param named after its node is no longer prefixed with the node kind a second time.

- e3d02e6: `boolean` `operation`, `raster_multimerge` `blend_mode` and `color_grade` `interpolation` are now real enums, so a value an enum doesn't have is an error instead of falling back to the first option. `color_grade` `interpolation` takes `"nearest"` or `"linear"` instead of `0` or `1`, and `shadow` `color` fails with a clear message when it is a gradient, which it used to replace with the gradient's first stop. `merge` `blend_mode` and the `sampling` of `resize` and `transform` now also accept expressions.
- ca87eaa: `@lumiscia/lumen-types` now generates every node and composition type from `composition.schema.json`, so node params carry their schema descriptions as JSDoc and the root types can no longer drift from the schema. Enums that several nodes share are one type (`BlendMode`, `LoopMode`, `MaskMode`, `ChannelSelector`, `MediaInSourceKind`) and the old per-node names are aliases of it. New exports are `AudioClip`, `AudioTrack`, `RenderSettings` and `Timeline`. `MetaManifest` is gone, and `Connection`, `CompositionTemplate` and `LumenComposition` are type aliases rather than interfaces.

  The composition schema defines `timeline` and `render_settings` once in `$defs` and gives `metadata`, `$schema` and `lumenSchemaVersion` root types: a non-string version or `$schema`, or a `metadata` that is not an object with a string `name`, is now rejected. The `fps` default of 30 moves from the schema to its description. Consumers that vendor the schema need the new copy.

## 0.3.1

No changes in this release.

## 0.3.0

### Minor Changes

- 2c68ade: Add an animatable opacity processing node with correct premultiplied and straight-alpha handling.
- 5726783: Render SVG path commands with adaptive curve flattening, including relative, smooth, quadratic, and closed subpaths.
- 577d694: Add expression animation builtins: `progress`, `tween`, the `in`/`out`/`in_out` quad, cubic, expo and back easings, `cubic_bezier` and `spring`, plus `s` and `ms` suffixes on numbers. `vec2` values now support `+`, `-` and scaling, and `lerp`/`tween` work on numbers, `vec2` values and colours. Colours are expression values (`#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`), so colour and paint params accept expressions. Compositions can declare reusable `functions`, such as `"rise(start)": "tween(vec2(0, 60), vec2(0, 0), start, 16, 'out_expo')"`, which expand where they're called. The builtin table, easing names and globals are published in the schema and as `expressionBuiltins`, `easingNames`/`EasingName` and `expressionGlobals` in the types.
- 3f45d73: The GPU renderer now renders unchanging parts of a composition once and reuses them: a node whose params hold no expression and that reads no media or shader time, fed only by such nodes, is neither bound nor dispatched again on later frames of its plan. Output frames are unchanged. Kept output lives in the plan's own textures, so it counts against the plan cache budget and is dropped with its plan, and recompiles, font loads and texture replacements render it again.

  `memo` now caches. Its `cache_id` and `allow_expressions` params are removed in favour of `key`, an expression evaluated at the frame being rendered: while it gives the same value as on the frame `memo` last rendered, `memo` shows that output and nothing only it reads is bound or dispatched. `"key": "=floor(frame / 4)"` runs a heavy shader on one frame in four, and `"=floor(frame / 2)"` animates on twos. What `memo` holds is the source as rendered on the first frame of each key value that is rendered, so pair it with a `time_remap` to `floor(frame / 4) * 4` when frames may render out of order. The snapshot belongs to the memo node and its time-remap path rather than to a plan, so fractional frames and upstream switches that bind another plan show it too; it is taken again once every plan holding it is evicted, and any edit to the composition drops it, since a key holds output across frames, never across edits. Bounds a `crop` publishes are kept with the pixels. A literal or missing `key` adds nothing to the caching above. From TypeScript, `composition.addNode({ type: "memo", params: { key: floor(frame.div(4)) } })`.

  `GpuCompositionRenderer::cached_pass_count` and `submitted_pass_count` report how many passes were skipped and submitted, and `lumen-local` prints the per-frame averages as `static_cached` and `passes`.

- 076bf72: Text nodes take `glyph_offset`, `glyph_scale`, `glyph_rotation` and `glyph_opacity`, evaluated for every glyph to stagger glyphs, words and lines apart. Inside them, expressions can read the `glyph`, `word` and `line` index of the glyph being drawn and the `glyph_count`, `word_count` and `line_count` of the text, such as `"glyph_opacity": "=tween(0, 1, 10 + glyph * 2, 12)"`; elsewhere those globals are an error. Each glyph scales and turns about the middle of its advance and line, before the text's own transform, and fades before its `opacity`. `@lumiscia/lumen-shared` exports them as `glyph`, `word`, `line`, `glyphCount`, `wordCount` and `lineCount`.
- 2b50b73: Add opt-in `render_mode: "msdf"` text rendering through the `msdf` Cargo feature (enabled by default). Raster and MSDF glyphs share a fixed 4096² RGBA8 atlas costing 64 MiB per renderer once text is used. Standalone GPU callers can install the atlas with `CompiledComposition::prepare_text_atlas`.
- 97f55bf: Harden runtime rendering and preview lifecycle cleanup, add configurable local font discovery, and make CUDA frame ownership explicit in the Rust API.
- 1b30cb7: Add animatable start and end trimming for path strokes while keeping fill geometry intact.
- b88aeda: Add typed `vec2` expression values for animating vector node parameters and referencing vector properties.
- 4e61e7a: `wgsl_shader` now takes up to 16 inputs (`input0..input15` plus a shared `input_sampler`), gives every shader `lumen.time`, `lumen.frame` and `lumen.resolution`, binds typed uniforms by name from the shader's uniform struct, rebuilds its pipeline when the source changes, premultiplies straight-alpha inputs before the shader sees them, and rejects unknown or malformed uniform values and shader bindings above 2 instead of ignoring them.

  Uniforms are set with a `uniforms` object instead of the `bindings` string, which is removed. Each value is a number, an array of 2-4 numbers or `=expression`s, a whole-value `=expression`, or, for `vec3<f32>` and `vec4<f32>` fields, a `#rrggbb` / `#rrggbbaa` colour or a colour expression such as `=rgb(255, 128, 0)`. `wgslShader()` values also take typed `Expr`s from the expression helpers. The schema and the generated `ShaderUniforms` / `ShaderUniformValue` types describe it, and `wgslShader()` in `@lumiscia/lumen-shared` declares the uniform struct in TypeScript and type-checks each field's value.

  The GPU renderer now keeps compiled plans within an estimated memory budget (1 GiB by default, configurable with `GpuCompositionRenderer::set_plan_cache_budget`), dropping least recently used plans instead of keeping one per animated switch or shader combination.

### Patch Changes

- d52b7d6: Allow `=` expressions in the schema and TypeScript types exactly where the engine accepts them. Paints, colours, named enums and `alpha_premultiply.mode` no longer type-check with an expression.
- b314e00: Render transformed media into the composition bounds so translation and rotation are not clipped to the source texture.
- f054aaf: Add `paint_supersample` for background and text nodes and `edge_antialias` for shape and path nodes.
- d658d6c: Emit a valid JSON schema by moving the node kind list to `x-lumen-nodeKinds`, and drop the unsupported `map` field from node types.

## 0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.
