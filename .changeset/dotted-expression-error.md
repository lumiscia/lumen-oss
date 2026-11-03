---
"@lumiscia/lumen-bindings": minor
---

An expression such as `title.opacity` now fails to parse with a message pointing at `node(id, 'property')`, instead of parsing and failing when it is evaluated. Reading a node property that has no expression value, such as a paint, now says `property `color` on node 1 can't be read in an expression`. Unknown node params no longer carry migration hints for the removed `wgsl_shader` `bindings` and `memo` `cache_id` params. For Rust embedders, `ExpressionContext::duration_frames`, `TrackId`, `ExpressionId`, `ExpressionReference`, the `resolve_*` property methods and the `expr` submodules are gone.
