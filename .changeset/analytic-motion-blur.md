---
"@lumiscia/lumen-bindings": minor
---

Shape, path, text and `transform` nodes take a `motion_blur` shutter angle in degrees, which smears the layer along its motion over a shutter centred on the frame. Expressions can now be evaluated between frames, so `frame` and `time` may be fractional during the shutter.
