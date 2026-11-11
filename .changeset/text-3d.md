---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`text_3d` extrudes text into a solid in a 3D scene. It shapes `content` as a `text` node with the same font and layout would, with its kerning, ligatures, font fallback and wrapping, then extrudes each glyph's outline `depth` deep along Z, centred on `position`, with front and back caps that keep counters open. `bevel_size` and `bevel_segments` round the edges off in a quarter-round profile, or a flat 45° chamfer at 1 segment; a glyph whose strokes are too thin for the bevel gets the widest one that fits it. It takes the `position`, `rotation`, `scale` and material params of a `shape_3d`.

`font_size` is in world units, the height of the em square, so at the default of 1 a word stands about as tall as a default `shape_3d` box is wide. `max_width`, `letter_spacing` and `line_height` are in world units too. `alignment_horizontal` and `alignment_vertical` align lines as in 2D and choose which point of the text's box sits at `position`, its centre by default.

Geometry is rebuilt only when the text, font, layout, depth or bevel change, and repeated glyphs share one mesh, so animating a `text_3d`'s transform or material costs no more than a `shape_3d`'s. A node draws at most as many glyphs as the host's text glyph limit allows a `text` node, and at most 4000000 vertices and 12000000 indices; more is a render error. Glyphs drawn from colour or bitmap tables, such as emoji, have no outline, so they are left out with a new `font.no_outlines` warning. `check` reports a family no font answers to as `font.fallback`, as for `text`. Rich text spans, decorations, strokes and the `glyph_*` params of `text` have no counterpart.

```json
{
  "nodes": [
    {
      "id": 1,
      "type": "text_3d",
      "params": {
        "content": "Lumen",
        "depth": 0.3,
        "bevel_size": 0.02,
        "rotation": [-20, 0, 0],
        "color": [232, 176, 96, 255]
      }
    },
    { "id": 2, "type": "light_3d" },
    { "id": 3, "type": "merge_3d" },
    { "id": 4, "type": "render_3d" },
    { "id": 5, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 3, "to_port": "objects" },
    { "from_node": 2, "to_node": 3, "to_port": "objects" },
    { "from_node": 3, "to_node": 4, "to_port": "scene" },
    { "from_node": 4, "to_node": 5, "to_port": "source" }
  ]
}
```
