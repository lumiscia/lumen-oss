---
"@lumiscia/lumen-bindings": patch
---

Gradient paints with `interpolation: "linear_srgb"` now blend their stops in linear light, as documented. They were mixed in sRGB like the default, so mid-gradient colours came out darker than asked for. `srgb` gradients are unchanged.
