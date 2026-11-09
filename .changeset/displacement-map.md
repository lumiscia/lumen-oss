---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

A `displacement_map` node moves each pixel of its `source` input by the pixel of its `map` input over it, in a single pass. `channel_x` and `channel_y` pick the map channel that drives each axis, `red`, `green`, `blue` or `luma`, and `amount_x` and `amount_y` are the most pixels it moves: a byte of 0 moves the source the whole amount left or up, 255 nearly as far right or down, and 128 leaves it in place. Map bytes are premultiplied by alpha, so transparent areas of the map, and everything outside it, displace nothing. `edge` is `clamp`, which repeats the source's edge pixels, or `transparent`, and `sampling` is `linear` or `nearest`. The output is sized to the source grown by the amounts where the map overlaps it.
