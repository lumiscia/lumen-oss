---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-cli": minor
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-types": minor
---

Render at a fraction of the composition's size. A render scale above 0 (1 by default, and above 1 too) makes frames `ceil(width * scale)` by `ceil(height * scale)` and look like the full render scaled, with every raster along the way that much smaller, so a preview or a draft render costs less. Lengths in params stay in composition pixels and expressions still see the composition's size: blurs scale their spread, text keeps its line breaks, images are reduced to the scale on load, `render_3d` sizes scale with `supersample` on top, and 3D world units don't change. A `wgsl_shader` sees the scaled size in `lumen.resolution` and the scale in the new `lumen.render_scale`.

```sh
lumen render film.json -o draft.mp4 --scale 0.5
```

In the preview, `renderScale` is a session input and `setRenderScale` changes it without reloading the composition. The canvas follows the scaled size, and the scale is kept across loads. The renderer's `outputWidth` and `outputHeight` give the size of rendered frames.

```ts
session.setRenderScale(0.5);
```
