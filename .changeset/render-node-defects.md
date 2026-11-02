---
"@lumiscia/lumen-bindings": minor
---

Several render node fixes. A `crop` now keeps its bounds through `exposure`, `levels`, `hue_saturation`, `opacity`, `color_grade` and the other per-pixel nodes, so a `resize` after them scales the cropped area instead of the whole canvas. `lumen-local` writes PNG files with straight alpha instead of premultiplied colour. A custom `wgsl_shader` must declare `@workgroup_size(8, 8, 1)`, and a `transform` with a pivot but without `custom_pivot: true` is now an error rather than a pivot that is sometimes ignored. `time_remap` keeps frames past 2^32 instead of wrapping to 0, and rejects a non-finite `frame`. Graphs built in Rust with inputs set directly on nodes are now checked for cycles, and a plan prepared on the GPU renderer can no longer be confused with another plan of the same shape.
