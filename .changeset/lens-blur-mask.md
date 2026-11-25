---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`lens_blur` takes an optional `mask` raster that keeps pieces sharp. Where the mask is 1 a pixel keeps its own colour and doesn't spread over its neighbours, where it is 0 the blur is unchanged, and in between the blur shrinks by that share. `mask_mode` picks the channel read, as `merge`'s does (`alpha`, `luma`, `red`, `green` or `blue`), so a `render_3d` `matte` channel can feed it directly. It costs nothing extra: the mask is read where depth is. This keeps lyric pieces crisp while the rest of the scene defocuses:

```ts
const render = n.render_3d({ scene, mattes: ["lyrics"], depth_near: 3, depth_far: 20 });
n.lens_blur({
  source: render.out.color,
  depth: render.out.depth,
  mask: render.out.matte,
  mask_mode: "red",
  radius: 16,
  focus: 0.18,
});
```
