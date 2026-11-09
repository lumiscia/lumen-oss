---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

A `glow` node adds a blurred copy of its source's brightest pixels to it, as light. Luminance above `threshold` is remapped to full strength at white, so pixels at or below it, and black at any threshold, cast nothing, and a `threshold` of 1 lets nothing glow; past 254/255 white dims too, so the glow fades out continuously as the threshold reaches 1. The light is blurred with the `blur` node's Gaussian of `radius`, scaled by `intensity` (up to 1000), and added to the premultiplied source, clamped at full. With `tint_enabled`, the glow takes the solid `tint` colour at the luminance of the light that casts it. `glow_only` outputs the glow alone, whose alpha is its brightest channel. The output is sized to the source grown by the blur's reach.
