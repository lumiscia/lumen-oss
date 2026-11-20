---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

The new `lens_distortion` node bends a raster the way a lens does, with radial coefficients `k1` and `k2` (positive `k1` is barrel distortion, negative pincushion), an optical `center` as a fraction of the frame, and `chromatic_aberration` that magnifies red and shrinks blue so fringes grow toward the corners. `direction: "undistort"` is the exact inverse of `"distort"`, for matching graphics to footage, and `fit: "fill"` zooms in until no edge is exposed. Distances are measured in half-diagonals of the frame, so it looks the same at any render scale.

```json
{
  "id": 2,
  "type": "lens_distortion",
  "params": { "k1": 0.15, "chromatic_aberration": 0.004 },
  "in": { "source": 1 }
}
```
