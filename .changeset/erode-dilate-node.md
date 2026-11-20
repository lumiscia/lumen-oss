---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

New `erode_dilate` node grows or shrinks the opaque part of a raster by `radius` pixels, so pulling a key's edge in, widening a matte before a blur, or outlining an already-rasterized letter no longer needs a custom shader. A positive radius grows alpha and a negative one shrinks it, with round or square corners (`shape`) and an optional feathered edge (`softness`). Grown pixels take the colour of the nearest opaque pixel, so the result composites without a dark fringe. The cost grows with the logarithm of the radius, not the radius.

```ts
nodes.erode_dilate({ radius: 3, source: matte });
```
