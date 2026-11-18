---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Add a `particles_3d` node that draws up to 100,000 copies of its `object` scene as particles placed on the GPU. With the defaults it scatters them once over its `surface` scene, as likely anywhere as anywhere else of the same area, which suits grass or rocks; a `rate`, `lifetime`, `speed`, `spread` and `gravity` make it an emitter. Particles are stateless, so any frame renders alone and the same, and a frame's CPU work doesn't grow with `count`.

```json
{
  "nodes": [
    { "id": 1, "type": "shape_3d", "params": { "shape": "plane", "size": [10, 1, 10] } },
    { "id": 2, "type": "shape_3d", "params": { "shape": "cylinder", "size": [0.03, 0.4, 0.03] } },
    {
      "id": 3,
      "type": "particles_3d",
      "params": { "count": 20000, "seed": 7, "align_to_normal": true, "scale": [0.6, 1.4] }
    }
  ],
  "connections": [
    { "from_node": 1, "to_node": 3, "to_port": "surface" },
    { "from_node": 2, "to_node": 3, "to_port": "object" }
  ]
}
```
