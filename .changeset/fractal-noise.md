---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

A `fractal_noise` source node generates opaque grey fractal noise on the GPU: 2D gradient noise, the per-pixel counterpart of the `noise()` expression builtin, summed over 1 to 8 `octaves` of halving size and strength. `scale` is the size of the largest features in pixels, `seed` picks an unrelated pattern, and `evolution` morphs it smoothly in turns, repeating every whole turn, so `"=time"` loops once a second. `mode` is `smooth` or `turbulent`, which folds each layer for dark creases. `contrast`, `brightness`, `offset_x` and `offset_y` adjust and move the pattern, and `width` and `height` size the output as on `background`. The noise is built from integer hashing, with its gradients and long offsets worked out on the CPU, so a frame looks the same on every GPU.
