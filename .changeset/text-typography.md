---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

The text node gains `text_transform`, `paragraph_spacing` and, on spans, `baseline_shift`.

`text_transform` is `none`, `uppercase`, `lowercase` or `capitalize`, on the node and on spans, where an unset span takes the node's. It maps the text before shaping with Unicode case mapping, so `ß` becomes `SS`. `capitalize` uppercases the first letter or digit of each word, and a word continues across spans. Glyph, word and line numbering, `span` indices, `text_width` and `text_height` count the transformed text, as do decoration extents, which are fractions of its bytes.

`baseline_shift` is a span's offset in pixels, positive up. It moves the span's glyphs, its own underline, strikethrough and highlight, and its gradient box, while the node's decorations that the span inherits stay on the line. The line keeps its height, so shifted text can overlap its neighbours, unlike CSS `vertical-align` with a length. `text_bounds` includes the shift, and `text_width` and `text_height` do not. Superscripts are a smaller `font_size` with a `baseline_shift`.

`paragraph_spacing` adds pixels after each paragraph, which a hard line break starts, but not after the last. Wrapped lines of one paragraph keep their line pitch. It adds to `text_height`.

Text that sets none of these renders as before.

For Rust: `lumen_text::TextSpan` has `transform` and `baseline_shift`, `TextLayoutRequest` has `paragraph_spacing`, `TextSegment` has `shift`, and `TextDecoration` has `shifted`. `ShapedText::bounds` can start above the first line when a span is raised. The generated type for the `text_transform` param is `TextTransform`, and a node param named after its node is no longer prefixed with the node kind a second time.
