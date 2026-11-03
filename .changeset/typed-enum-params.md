---
"@lumiscia/lumen-bindings": minor
---

`media_in.kind`, `media_in.loop_mode`, `boolean.operation` and `raster_multimerge.blend_mode` now hold their enum rather than an integer. Names parse as before and expressions are still accepted, but an expression whose value isn't in the enum, such as `"kind": "=2"`, now fails with an error naming the node and param instead of falling back to the first option. `merge.blend_mode` still takes names only, and an expression there now fails with `does not support expressions`.
