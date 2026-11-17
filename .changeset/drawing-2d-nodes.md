---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

New 2D vector nodes draw any number of paths and shapes in one GPU pass. They use two new port kinds: `geometry`, an outline, and `drawing`, styled outlines.

- `path_2d` and `shape_2d` make geometry, and `offset_2d` grows or shrinks it.
- `fill_2d` and `stroke_2d` turn geometry into a drawing.
- `shadow_2d` puts soft shadows under a whole drawing, and `merge_2d` stacks drawings and places them.
- `render_2d` turns a drawing into a raster.

The `path`, `shape` and `raster_multimerge` nodes are unchanged.

A stroked, filled path with a shadow:

```json
{
  "nodes": [
    { "id": 1, "type": "path_2d", "params": { "data": "M 0 0 L 120 20 L 40 100 Z" } },
    { "id": 2, "type": "fill_2d", "params": { "paint": [255, 120, 0, 255] } },
    { "id": 3, "type": "stroke_2d", "params": { "paint": [0, 0, 0, 255], "width": 4 } },
    { "id": 4, "type": "merge_2d", "params": { "position": [200, 120] } },
    {
      "id": 5,
      "type": "shadow_2d",
      "params": { "shadows": [{ "offset": [6, 10], "radius": 12, "opacity": 0.4 }] }
    },
    { "id": 6, "type": "render_2d" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 2, "to_port": "geometry" },
    { "from_node": 1, "to_node": 3, "to_port": "geometry" },
    { "from_node": 2, "to_node": 4, "to_port": "drawings" },
    { "from_node": 3, "to_node": 4, "to_port": "drawings" },
    { "from_node": 4, "to_node": 5, "to_port": "drawing" },
    { "from_node": 5, "to_node": 6, "to_port": "drawing" }
  ]
}
```
