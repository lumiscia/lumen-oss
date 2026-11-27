---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

The params that only ever took a solid colour now take a plain colour instead of a paint: `shadow`'s `color`, `halftone`'s and `ordered_dither`'s `ink` and `paper`, `chroma_key`'s `key_color`, `glow`'s `tint`, a `shadow_2d` shadow's `color`, `material_3d`'s and `light_3d`'s `color`, `render_3d`'s `fog_color` and `instance_3d`'s `tint`. They accept the same values as before (`[r, g, b]`, `[r, g, b, a]`, `#rrggbb`, `#rrggbbaa` and a colour expression) and render the same. A gradient there is now an invalid type that `check` reports at the param, as for any other colour, where the 2D ones used to pass `check` and fail only when the frame rendered. The schema and the TypeScript types say so: these params are `ExpressionValue<Color>`, not `ExpressionValue<Paint>`. A literal one can also be read by another node's expression with `node(id, "param")`.
