---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`merge_3d` gets a `visible` param. While it is false the merge and everything wired into it is out of the scene, objects, lights and cameras alike, and nothing reached only through it is evaluated, drawn or decoded. It takes an expression, so one `render_3d` can hold several shots and show each over its own frames.

```json
{ "id": 4, "type": "merge_3d", "params": { "visible": "=frame >= 108 && frame < 203" } }
```
