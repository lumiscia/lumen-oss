---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`directional_blur`, `radial_blur` and `zoom_blur` nodes average their source along a path through each pixel, centred on it. `directional_blur` spreads each pixel along a line `distance` pixels long at `angle` degrees clockwise from horizontal. `radial_blur` spins it along an arc of `amount` degrees, up to 360, about `center`, and `zoom_blur` streaks it along the ray from `center` over `amount` times its distance from it. `edge` is `clamp`, which repeats the canvas's edge pixels, or `transparent`. Each blur takes about one sample per pixel of its longest streak, in two passes of at most 64 taps each, so streaks up to 4096 pixels long stay smooth. The output is sized to the source grown along the path it is blurred over.
