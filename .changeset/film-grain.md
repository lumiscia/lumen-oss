---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

A new `film_grain` node adds grain to a raster, so an edit no longer needs a custom shader or baked "flecks" for it. The grain is a pattern fixed to the canvas that stays put as the source moves and looks the same at any render scale. `amount` is its strength as a standard deviation in sRGB-encoded units, `size` its grain size in composition pixels, `roughness` mixes in a coarser grain for a paper-like tooth, `response` makes the strength follow the brightness so that 1 leaves black and white clean, and `monochrome` chooses one grain for all channels or one each. A `seed` of `"=frame"` changes the grain every frame.

```json
{
  "type": "film_grain",
  "params": { "amount": 0.12, "size": 1.5, "response": 0.7, "seed": "=frame" }
}
```
