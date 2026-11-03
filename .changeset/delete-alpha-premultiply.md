---
"@lumiscia/lumen-bindings": minor
---

Removed the `alpha_premultiply` node. Every raster is now premultiplied, so there is no straight-alpha state for a composition to convert into, and the filters, `merge`, `raster_multimerge`, `boolean`, `blur`, `shadow`, `transform` and `wgsl_shader` no longer branch on one. A composition that still uses `alpha_premultiply` is rejected with a message.
