---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Add `extrude_3d` and `lathe_3d`, which turn a 2D `geometry` outline, a `path_2d` or `shape_2d`, into a solid. `extrude_3d` gives the outline `depth` along Z, with an optional bevel as on `text_3d`; `lathe_3d` spins it around the world Y axis in `segments` steps over a `sweep` of up to 360 degrees. Both take a `material_3d` through `material` and the `position`, `rotation` and `scale` of a `shape_3d`.

The geometry's pixel coordinates, after its own transform, become world units at `units_per_pixel` each (0.01 by default), with Y flipped, so pixel (0, 0) is the origin the node places the solid about. A `lathe_3d` profile's X is its distance from the axis and its Y its height. A closed contour makes a ring solid and an open one a surface; a point on the axis is a pole with no degenerate triangles. An `offset_2d` in the geometry is not supported yet and fails the render, naming the node. Meshes are built once and kept while their outline and params stay the same, and share the `text_meshes` cache budget.

```json
{
  "nodes": [
    {
      "id": 0,
      "type": "path_2d",
      "params": {
        "data": "M 50 0 L 61 35 L 98 35 L 68 57 L 79 91 L 50 70 L 21 91 L 32 57 L 2 35 L 39 35 Z",
        "position": [-50, -45]
      }
    },
    { "id": 1, "type": "material_3d", "params": { "color": [232, 176, 96, 255], "metallic": 0.8 } },
    {
      "id": 2,
      "type": "extrude_3d",
      "params": { "units_per_pixel": 0.02, "depth": 0.2, "bevel_size": 0.03 }
    },
    {
      "id": 3,
      "type": "path_2d",
      "params": {
        "data": "M 0 0 L 60 0 C 105 -40 105 -100 55 -150 L 45 -195 L 38 -195 L 46 -150 C 90 -100 90 -45 50 -10 L 0 -10 Z"
      }
    },
    { "id": 4, "type": "lathe_3d", "params": { "segments": 64, "position": [2, 0, 0] } }
  ],
  "connections": [
    { "from_node": 0, "to_node": 2, "to_port": "geometry" },
    { "from_node": 1, "to_node": 2, "to_port": "material" },
    { "from_node": 3, "to_node": 4, "to_port": "geometry" }
  ]
}
```
