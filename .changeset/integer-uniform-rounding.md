---
"@lumiscia/lumen-bindings": patch
---

`wgsl_shader` integer uniforms accept numbers a rounding error away from a whole number, such as an expression giving 2.9999999999999996 for a `u32`.
