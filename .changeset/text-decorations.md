---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

The text node draws underlines, strikethroughs and highlights. The new node-level `underline`, `strikethrough` and `highlight` params are off when absent and apply to string content and to every span that leaves its own unset. A span's own decoration replaces the node's, and `null` turns it off for that span.

Consecutive spans that take the node's decoration form one run, which draws one line or box per line of text, so mixed font sizes don't step or seam. A line's absent `thickness` and `offset` come from the tallest span on that line, and its absent `paint` is the text's own paint. Highlights break per line like CSS `box-decoration-break: clone`: each line gets its own padding and corner radius. `extent` maps `0..1` along the run's text in reading order, continuing across line breaks and following right-to-left text, so `"extent": "=vec2(0, clamp(time * 2, 0, 1))"` draws an underline in across wrapped lines. Lines can be `solid`, `double`, `dotted`, `dashed` or `wavy`.

Decorations are painted in CSS order (highlights, underlines, glyphs, then strikethroughs). They follow the node's transform, opacity and motion blur but not the `glyph_*` params. They never change the text's layout or `text_width` and `text_height`, and they count toward the text node's glyph limit. Text without decorations renders as before.

For Rust: `TextParams` has `underline`, `strikethrough` and `highlight` fields. `GpuGlyphInstance` has a `GLYPH_MODE_RECT` render mode for decoration rects, made with `GpuGlyphInstance::rect`, and `ShapedText::decorate` lays out a `TextDecoration`. Node params can now be `Option<T>`, unset unless given, and their specs then have no default value. The generated `NodeParamSpec.defaultValue` is optional.
