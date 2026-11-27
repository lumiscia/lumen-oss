---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

**Breaking:** the raster `path` and `shape` nodes are removed, with their `repeat` param and its `Repeat` type. A composition that uses them fails `check` with `node.unknown_type`. Draw outlines with the drawing nodes instead: an outline node, a style node for each fill and stroke, and a `render_2d` that rasterizes them in one pass. Every param has a counterpart:

| `path` / `shape`                                              | Drawing nodes                                                                                                                                                                                                                                                                                    |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `data`; `geometry_kind`, `width`, `height`, `border_radius`   | The same params on `path_2d`; on `shape_2d`                                                                                                                                                                                                                                                      |
| `position`, `scale`, `rotation`, `anchor`                     | The same params on `path_2d` or `shape_2d`, or on a `merge_2d` to place several drawings together                                                                                                                                                                                                |
| `fill_enabled`, `fill_paint`                                  | A `fill_2d` of the outline, with `paint`                                                                                                                                                                                                                                                         |
| `stroke_enabled`, `stroke_paint`, `stroke_width`              | A `stroke_2d` of the outline, with `paint` and `width`                                                                                                                                                                                                                                           |
| `trim_start`, `trim_end`, `stroke_dash`, `stroke_dash_offset` | `trim_start`, `trim_end`, `dash` and `dash_offset` on `stroke_2d`                                                                                                                                                                                                                                |
| `offset`                                                      | An `offset_2d` between the outline and its `fill_2d`                                                                                                                                                                                                                                             |
| `feather`, `edge_antialias`                                   | `feather` and `edge_antialias` on `fill_2d`. Strokes are always antialiased                                                                                                                                                                                                                      |
| `opacity`                                                     | An `opacity` node after the `render_2d`. `opacity` on each `fill_2d` and `stroke_2d` looks the same only where they don't overlap: where a fill and its stroke overlap, both fade and the overlap shows through more                                                                             |
| `motion_blur`                                                 | A `motion_blur` node around the `render_2d`                                                                                                                                                                                                                                                      |
| `repeat`                                                      | An `instance_2d` between the styles and `render_2d`, whose `position`, `scale`, `rotation` and `opacity` read `instance`. Its copies turn and scale about `anchor`, measured from the drawing's origin, so set `anchor` to the outline's `position` plus its `anchor` to turn each copy in place |
| The node's raster output                                      | The `render_2d`'s output, which goes wherever the node's did                                                                                                                                                                                                                                     |

A filled and stroked rectangle:

```json
{
  "id": 1,
  "type": "shape",
  "params": {
    "width": 40,
    "height": 24,
    "position": [8, 8],
    "fill_paint": "#ff6a00",
    "stroke_enabled": true,
    "stroke_width": 2,
    "stroke_paint": "#ffffff"
  }
}
```

is now

```json
{ "id": 10, "type": "shape_2d", "params": { "width": 40, "height": 24, "position": [8, 8] } },
{ "id": 11, "type": "fill_2d", "params": { "paint": "#ff6a00" }, "in": { "geometry": 10 } },
{ "id": 12, "type": "stroke_2d", "params": { "paint": "#ffffff", "width": 2 }, "in": { "geometry": 10 } },
{ "id": 13, "type": "merge_2d", "in": { "drawings": [11, 12] } },
{ "id": 1, "type": "render_2d", "in": { "drawing": 13 } }
```
