---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

New `instance_2d` node draws many copies of a drawing, as `instance_3d` does for scenes. Each copy's `position`, `scale`, `rotation`, `anchor`, `opacity` and `tint` are evaluated per copy and can read `instance` and `instance_count`, or a literal `copies` list places them by hand. Copies that differ only in where they sit share one flattened outline, so a thousand moving copies of a path cost a drawable each. Nested `instance_2d` nodes multiply their counts, up to 8 deep and 16384 drawables in all:

```json
{
  "id": 3,
  "type": "instance_2d",
  "params": { "count": 50, "position": "=vec2((instance % 10) * 40, floor(instance / 10) * 40)" },
  "in": { "drawing": 2 }
}
```
