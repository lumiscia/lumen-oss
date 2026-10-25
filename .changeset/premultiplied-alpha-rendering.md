---
"@lumiscia/lumen-bindings": patch
---

Treat rasters as premultiplied alpha consistently. Merges no longer darken translucent overlays, shape and path strokes composite over fills correctly, gradients interpolate in premultiplied space, and colour adjustments keep fully transparent pixels transparent.
