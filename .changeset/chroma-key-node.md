---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

New `chroma_key` node removes a green or blue screen from a raster. It measures each pixel's distance from `key_color` in chromaticity, the colour divided by its brightness, so a screen keys evenly from its shadows to its highlights. Pixels within `tolerance` are removed, `softness` feathers the edge back to fully kept, and `spill` pulls the screen's cast out of the pixels that remain. `matte_only` outputs the matte as white over transparent, to feed `merge`'s mask or `channels`.

```json
{
  "type": "chroma_key",
  "in": { "source": 1 },
  "params": { "key_color": [0, 177, 64, 255], "tolerance": 0.1, "softness": 0.1, "spill": 0.7 }
}
```
