---
"@lumiscia/lumen-bindings": patch
---

`render_2d` draws the copies of an `instance_2d`, and a `merge_2d` that places one drawing at several positions, from one shared drawable and a small record per copy, instead of a full drawable per copy. At 1000 moving copies of a star, binding a frame takes about a third less CPU time and the GPU about a quarter less time. Drawings look exactly as before.
