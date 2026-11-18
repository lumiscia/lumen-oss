---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Add a `copies` param to `instance_3d`: a literal list of `{ position, rotation, scale, tint }` objects that places hand-set copies without an expression per param per copy. A field left out takes the default of the param of that name. The list and its numbers can't be expressions and it holds at most 100000 copies. It is used in place of `count`, `position`, `rotation`, `scale` and `tint`, and `check` fails, naming the node and the params, if any of them is set beside a list that has copies. The list is placed once and kept on the GPU like static expression placements, so a moving camera or parent costs nothing per copy, and `lumen-types` gains `InstanceCopies` and `InstanceCopy`.

```json
{
  "id": 4,
  "type": "instance_3d",
  "in": { "scene": 3 },
  "params": {
    "copies": [
      { "position": [0, 0, 0], "scale": [1, 2, 1], "tint": "#ff8040" },
      { "position": [2, 0, 1], "rotation": [0, 45, 0] }
    ]
  }
}
```
