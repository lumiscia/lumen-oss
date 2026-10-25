---
"@lumiscia/lumen-bindings": patch
---

Treat rasters as premultiplied alpha consistently. Merges no longer darken translucent overlays, shape and path strokes composite over fills correctly, gradients interpolate in premultiplied space, and colour adjustments keep fully transparent pixels transparent. Compositing nodes and the default WGSL shader now honour straight-alpha inputs, and the Alpha Premultiply mode must be a constant rather than an expression.
