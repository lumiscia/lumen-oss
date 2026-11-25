---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`render_3d` has a `matte` output that selects objects after rendering, so keeping lettering sharp under a blur or grading one object no longer takes a second render and a difference of the two. Scene sources and `merge_3d`, `instance_3d` and `particles_3d` take a `matte` name, the innermost winning, and `render_3d`'s `mattes` lists up to four names, one for each of the output's red, green, blue and alpha channels. A channel holds the visible coverage of its objects, composited like colour: a surface covers by its alpha, and one in front hides what is behind by its own. The matte is drawn in the colour's draw and averaged down with it by `supersample`, so its edges line up with the colour's. `merge` takes `mask_mode` `red`, `green` and `blue` to read one channel:

```ts
const render = n.render_3d({ scene, mattes: ["title"] });
const { color, matte } = render.out;
n.merge({
  base: n.blur({ source: color, radius: 12 }),
  overlay: color,
  mask: matte,
  mask_mode: "red",
});
```
