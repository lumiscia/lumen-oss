---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

`image_plane_3d` takes `region`, `[x, y, width, height]` in its source's pixels, to show only that part of a raster on a rectangle that covers just that part.

A plane with a region is where it would be and as big as it would be with the whole source on it, so it looks as a plane showing a `crop` of that region does and the two can be swapped. `width` is still the width of the whole source, and `position`, `rotation` and `scale` pivot about the whole source's centre. A region is cut off at the source's edges as `crop`'s is. Each number takes an integer or an expression; the whole `region` cannot be an expression. Unset shows the whole source, as before.

Planes with regions of one source share its texture and its mip levels and draw together, so a region costs nothing beyond the plane, even one that changes every frame. A `crop` before each plane makes a raster the size of the source and a draw for every piece, on every frame it changes. Cutting a 1080p raster into 63 planes at 120 frames takes 0.6 s, holds 204 MB and submits 2 passes a frame, where 63 crops took 7.2 s, held 1.2 GB and submitted 64.

```json
{
  "id": 4,
  "type": "image_plane_3d",
  "params": { "width": 16, "region": [320, 180, 640, 360], "position": [0, 0, -2] }
}
```

Sampling is kept inside the region, so what is outside it isn't meant to show, however small the plane is drawn. The price is under strong minification: the band along each edge of the region that is inside the source, and all of a narrow region, is sampled from a sharper mip level than its footprint, because a texel of a shared level can hold pixels from both sides of the edge. That band can alias or shimmer where a `crop` would be smooth.
