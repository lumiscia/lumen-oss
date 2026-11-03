---
"@lumiscia/lumen-bindings": patch
---

`color_grade` now fails with a clear error when LUT text holds `nan`, `inf` or a number too large for a float, instead of sending NaN to the GPU or silently clamping infinity. Finite LUTs render the same.
