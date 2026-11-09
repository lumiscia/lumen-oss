---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`directional_blur`, `radial_blur` and `zoom_blur` nodes average their source along a path through each pixel, centred on it. `directional_blur` spreads each pixel along a line `distance` pixels long at `angle` degrees clockwise from horizontal. `radial_blur` spins it along an arc of `amount` degrees, up to 360, about `center`, and `zoom_blur` streaks it along the ray from `center` over `amount` times its distance from it. `edge` is `clamp`, which repeats the canvas's edge pixels, or `transparent`. `zoom_blur` samples its streak in equal ratios of scale, so it reaches farther away from `center` than towards it. Each blur takes about one sample per pixel of its longest streak, up to 4096, so streaks up to 4096 pixels long stay smooth; streaks longer than about 256 pixels are taken in two passes whose second resamples the first, which softens sharp edges by a few levels more than single samples would, and streaks from 64 to 256 pixels crossfade between the two so that animating a blur never jumps. The output is sized to the source grown along the path it is blurred over.
