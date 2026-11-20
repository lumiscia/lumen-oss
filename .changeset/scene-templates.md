---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

A template in `compositions` can output a 3D scene, so one rig, such as a glyph card or a camera rig, is written once and placed many times with its own props. A template whose `media_output` reads a scene, such as a `merge_3d`, makes every `clip` playing it a scene node for a `merge_3d`'s `objects` or a `render_3d`'s `scene`. Each clip's props and timing apply as they do for an image template, instances share the meshes and textures that come out the same, `matte` names inside the template apply, and a scene capture reports each instance's objects with the template and clips they are in. Wiring a scene template's clip into a raster input, or an image template's into a scene input, fails `check` with an error naming the template and both kinds. In TypeScript, `defineComposition` makes a scene template when `build` returns a scene, and its `clip()` fits scene inputs only.

```ts
const glyphCard = defineComposition({
  name: "glyph_card",
  props: { glyph: prop.string("A"), position: prop.vec3() },
  build: (_, p) =>
    n.merge_3d({ position: p.position, objects: [n.shape_3d(), n.text_3d({ content: p.glyph })] }),
});

n.render_3d({
  scene: n.merge_3d({
    objects: [
      glyphCard.clip({ props: { glyph: "L", position: [-1.5, 0, 0] } }),
      glyphCard.clip({ props: { glyph: "U", position: [1.5, 0, 0] } }),
    ],
  }),
});
```
