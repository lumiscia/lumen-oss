---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

The text node can outline its glyphs. The new node-level `stroke` param is off when absent and applies to string content and to every span that leaves its own unset. A span's own `stroke` replaces the node's, and `null` turns it off for that span. A stroke has a `paint`, a `width` in pixels and an `align`, and each field takes a literal or an `=expression`:

```json
{ "stroke": { "paint": [0, 0, 0, 255], "width": "=4 + 2 * sin(time * 6)", "align": "outside" } }
```

`outside`, the default, draws the outline behind every glyph's fill, reaching `width` pixels past the glyph's edge, as video captions do. The fill covers the outline's inner half. `center` draws the outline over the fills, centred on the glyph's edge, like `-webkit-text-stroke`. Joins are round. The stroke's paint respects `paint_space` the way a span's `color` does. Strokes follow their glyph's `glyph_*` transform and fade, and the node's transform, opacity and motion blur. Decorations are not stroked.

Distance-field glyphs draw their stroke from a true distance field in the text shader, at the fill's resolution, whose distance range doubles as often as the stroke needs, so animated widths stay smooth and share fields. Raster glyphs draw a stroked outline, keyed by its half width rounded to a quarter pixel up to 4 px and otherwise to a sixteenth of its power of two, so animated widths reuse them. A raster stroke too large for the glyph atlas is drawn from a distance field instead, and a stroke whose field is too large for the atlas is dropped. In `auto` mode each glyph's stroke takes the path its glyph does. `glyph_opacity` fades a glyph's stroke and fill as one, so a faded glyph's fill still hides the stroke under it. Neighbouring glyphs fade separately, so where one's stroke overlaps another both show. Shrink-to-fit scales stroke widths with the text. Strokes count toward the text node's glyph limit, and `text_bounds` and motion blur cover them. They don't change the layout or `text_width` and `text_height`. Colour and bitmap glyphs are not stroked. Text without a stroke renders as before.

For Rust: `TextParams` has a `stroke` field and `TextSpan` a `stroke` override, both of the new `TextStroke` kind. `TextSystem::prepare_stroked_glyphs` takes a half width per span and returns the stroke instances in `PreparedGlyphs::strokes`, each outlining the glyph instance `PreparedGlyphs::stroke_glyphs` names. `GpuGlyphInstance::color` is replaced by `opacity`, since the shader only read its alpha, and a `partner` index used by the new `GLYPH_PARTNER` flag.
