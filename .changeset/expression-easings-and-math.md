---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-shared": patch
"@lumiscia/lumen-types": patch
---

Expressions gain the `sine`, `quart`, `quint`, `circ`, `elastic` and `bounce` easing families (`in_`, `out_` and `in_out_` each, e.g. `out_bounce(t)` or `tween(0, 1, 0, 30, 'in_out_elastic')`), and the functions `sign`, `atan2`, `hypot` and `remap`. `@lumiscia/lumen-shared` exports a helper for each.
