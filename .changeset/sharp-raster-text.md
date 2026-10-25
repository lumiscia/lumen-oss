---
"@lumiscia/lumen-bindings": patch
---

Draw raster text on whole pixels so it stays sharp at fractional positions, keep glyph memory bounded when font sizes animate, warn instead of silently dropping glyphs when the atlas is full, measure relative text gradients against the text's own bounds, render colour emoji without dark fringes, and make `textWidth` use the same fonts as text rendering.
