---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

Two print looks are now nodes. `halftone` redraws a raster as a grid of round dots or lines in `ink` on `paper`: each cell takes the darkness at its centre and covers that fraction of its area, so a flat colour gives uniform dots, and `angle` rotates the grid. `ordered_dither` quantizes to `levels` with a `bayer2`, `bayer4` or `bayer8` threshold matrix, either as `ink` and `paper` or per colour channel. Both keep the source's alpha and anchor their pattern to the canvas origin, so it holds still when the source moves and when the render scale changes.

```json
{
  "type": "halftone",
  "params": { "pitch": 55, "angle": -19, "ink": [110, 28, 38, 255], "paper": [120, 134, 150, 255] }
}
```
