---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`alignment_vertical` of `text`, `text_2d` and `text_3d` takes `baseline`, which puts the node's `position` on the first line's baseline. Texts in different fonts at the same position now stand on one baseline, where `top` puts it at a different depth for each font's metrics.

```json
{
  "type": "text_2d",
  "params": {
    "content": "Hello",
    "font_family": "Inter",
    "position": [40, 300],
    "alignment_vertical": "baseline"
  }
}
```
