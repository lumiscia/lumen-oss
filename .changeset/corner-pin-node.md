---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

New `corner_pin` node pins a raster's four corners to four points of the canvas with perspective, the way to put footage or graphics on a screen, sign or page in a plate. `top_left`, `top_right`, `bottom_right` and `bottom_left` are canvas pixels, running clockwise from the top left. The source is filtered for how far the pin shrinks it at each pixel, so the far end of a steep pin doesn't alias, and the quad's edges are antialiased over transparency. Corners that don't make a convex quad fail the render, naming the corner at fault. `motion_blur` and `sampling` work as on `transform`, with animated corners smearing along their motion.

```json
{
  "type": "corner_pin",
  "in": { "source": 1 },
  "params": {
    "top_left": [412, 210],
    "top_right": [905, 262],
    "bottom_right": [893, 540],
    "bottom_left": [405, 498]
  }
}
```
