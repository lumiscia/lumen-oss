---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-types": minor
---

The text node's `content` takes styled spans as well as a string. Each span's font family, size, weight, style, letter spacing and color override the node's, and anything a span leaves out comes from the node. Span text and every span field can be an `=expression`. String content, including `=expressions`, renders as before. Span decorations parse but are not drawn yet.

Each span is drawn in its own `color` paint. The new `paint_space` param (`span` by default, or `text`) sets whether a span's own gradient covers just that span or the whole text. The node's `color`, which spans without a color use, always covers the whole text. `paint_supersample` still applies to the whole node.

The `glyph_*` params can read the new `span` and `span_count` expression globals, so they can target a single span, as in `=if(span == 1, vec2(0, -10 * sin(time * 6 + glyph)), vec2(0, 0))`. `@lumiscia/lumen-shared` exports them as `span` and `spanCount`, and a text node's `content` takes a string `Expr`. `text_width(node(id))` and `text_height(node(id))` measure the spans, and `node(id, "content")` reads span content as its joined text when that text is literal.

For Rust: `TextParams::content` is now a `TextContent`, and `TextParamsDelegate::content` is now a `TextContentDelegate`, so wrap string content in `TextContentDelegate::Text`. `GpuGlyphInstance` packs a paint index into the upper bits of `mode` (read and set with `render_mode()`, `paint()` and `set_paint()`), and `GpuTextGlobals` no longer has `paint_bounds`, since each paint now carries its own bounds.
