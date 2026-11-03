---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`boolean` `operation`, `raster_multimerge` `blend_mode` and `color_grade` `interpolation` are now real enums, so a value an enum doesn't have is an error instead of falling back to the first option. `color_grade` `interpolation` takes `"nearest"` or `"linear"` instead of `0` or `1`, and `shadow` `color` fails with a clear message when it is a gradient, which it used to replace with the gradient's first stop. `merge` `blend_mode` and the `sampling` of `resize` and `transform` now also accept expressions.
