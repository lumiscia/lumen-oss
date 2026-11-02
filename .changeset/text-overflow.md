---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

The text node can limit its text to a number of lines or a box, and shrink text to fit. The new `max_lines` and `max_height` params drop the lines past them, and are unlimited when absent. `overflow` sets how the last line shown ends: `clip`, the default, leaves it as it is, and `ellipsis` ends it with `…`. Clusters at that line's end, and whitespace before the ellipsis, are dropped until it fits `max_width`. The ellipsis goes at the line's end in reading order, so it sits on the left of right-to-left text, and it takes the style, paint and decorations of the text it follows.

`fit: "shrink"` scales every font size, the node's and each span's, by one factor of at most 1, so the text fits `max_width`, `max_lines` and `max_height` without breaking a word. Letter spacing, a pixel `line_height`, and decoration thickness, offset, padding and radius scale with it. The factor is the largest that fits, down to the one that takes `font_size` to `min_font_size` (8 by default); lines that still don't fit there are dropped by `overflow`. A fit takes at most 12 shapes, and is kept for the text's layout, so measuring, bounding and drawing the same text share one fit, and constant text pays for it once.

`text_width`, `text_height`, bounded output and decorations all cover the text as drawn, after truncation and fit. The `glyph_*` params count only the glyphs and lines shown. Glyphs keep their numbers, and the ellipsis is one glyph numbered after the last. Text that sets none of these params lays out as before.

For Rust: `TextParams` has `max_lines`, `max_height`, `overflow`, `fit` and `min_font_size` fields. `TextLayoutRequest` has `max_lines`, `max_height`, `overflow` and `min_fit_scale`, and `ShapedText::fit_scale` gives the factor a fit chose.
