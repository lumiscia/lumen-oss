---
"@lumiscia/lumen-bindings": minor
---

Removed the `curves` node. It ran the same table as `color_grade` with linear sampling, which is `color_grade`'s default `interpolation`, so renaming the node to `color_grade` and `curve_source` to `lut_source` renders the same pixels. A composition that still uses `curves` is rejected with that instruction.
