---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Add a `tint` param to `instance_3d`: a solid colour that multiplies the base colour of every object in a copy, texture or base colour map included, and the light the object gives off. It is white by default, and like `position`, `rotation` and `scale` it can be an expression evaluated for each copy, reading `instance` and `instance_count`. The tint's alpha is ignored, so a copy stays opaque or blended as its material makes it, and the tints of nested `instance_3d` nodes multiply. A still field with a literal or static tint keeps its tints on the GPU like its placements, and a tint that reads the frame is evaluated every frame.

```json
{
  "nodes": [
    { "id": 1, "type": "shape_3d", "params": { "size": [0.6, 0.6, 0.6] } },
    {
      "id": 2,
      "type": "instance_3d",
      "params": {
        "count": 10,
        "position": "=vec3(instance * 1.2 - 5.4, 0, 0)",
        "tint": "=rgb(instance / instance_count * 255, 140, 255 - instance / instance_count * 255)"
      }
    }
  ],
  "connections": [{ "from_node": 1, "to_node": 2, "to_port": "scene" }]
}
```
