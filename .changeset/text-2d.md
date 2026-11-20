---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

Text can enter the 2D drawing nodes. A new `text_2d` node outputs its glyphs' outlines as `geometry`, shaped as a `text` node with the same font, size and layout shapes them, so one title can feed a `fill_2d`, a `stroke_2d`, an `offset_2d` ring and a `shadow_2d` at once and draw in the same `render_2d` pass as the shapes around it, and `extrude_3d` and `lathe_3d` take it like a `path_2d`. Overlapping contours, as in variable fonts, fill as one, and the text is shaped again only when its text, font or layout change.

```json
{ "id": 1, "type": "text_2d", "params": { "content": "Lumen", "font_size": 240, "position": [960, 540], "alignment_horizontal": "center", "alignment_vertical": "middle" } },
{ "id": 2, "type": "fill_2d", "params": { "paint": "#ffffff" }, "in": { "geometry": 1 } },
{ "id": 3, "type": "stroke_2d", "params": { "paint": "#ee5b24", "width": 6 }, "in": { "geometry": 1 } }
```
