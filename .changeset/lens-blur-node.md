---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

New `lens_blur` node blurs like an out-of-focus lens: every point spreads into an evenly lit disc, or a polygon with `blades`, so highlights become bokeh instead of the soft glow of `blur`. With an optional `depth` raster, such as `render_3d`'s `depth` output, it gives depth of field: pixels within `focus_range` of `focus` stay sharp and the rest blur more the further they are, up to `radius`. A blurred foreground spreads over a sharp background, but a blurred background stays behind a sharp foreground's edges, so there are no halos where near and far meet. Any greyscale raster works as depth, so a gradient gives tilt-shift. The cost stays flat as the radius grows.

```ts
n.lens_blur({
  source: render.out.color,
  depth: render.out.depth,
  radius: 16,
  focus: 0.2,
});
```
