---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

`image_plane_3d` takes `sampling`, `"linear"` by default as before, or `"nearest"` to show pixel art in a 3D scene unfiltered. A nearest plane gives each pixel the one texel under it: magnified, texels stay hard-edged squares, and shown smaller, a pixel picks a texel instead of averaging the ones it covers, so it aliases where `linear` is smooth. A raster shown only by nearest planes gets no mip chain.

```json
{
  "id": 4,
  "type": "image_plane_3d",
  "params": { "width": 4, "sampling": "nearest" },
  "in": { "source": 3 }
}
```
