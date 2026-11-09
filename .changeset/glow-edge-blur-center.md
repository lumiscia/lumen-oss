---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`glow` takes an `edge` param like the blurs: `clamp`, the default, repeats the canvas's edge pixels as glow always has, and `transparent` lets the glow fade out towards the canvas edges. `radial_blur` and `zoom_blur` spin and zoom about the middle of the canvas, `[width / 2, height / 2]`, when `center` is left out, on any canvas size; before, they turned about pixel `[960, 540]`, which is off-centre on any canvas but 1920×1080.
