---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

The new `channels` node moves channels between rasters. Each of `red`, `green`, `blue` and `alpha` picks a channel of `source`, a channel of the optional `other` input, the luma of either, `zero` or `one`, and defaults to the same channel of `source`. Colour is taken unpremultiplied and the result is premultiplied by the output alpha. Nothing is encoded or decoded, so depth, normal and matte rasters keep their values. Choosing an `other_*` value with nothing connected to `other` is a check error on that param.

Connect `source` and `other` as usual:

```json
{
  "id": 3,
  "type": "channels",
  "params": { "alpha": "other_green" }
}
```
