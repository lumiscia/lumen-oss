---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

Small 3D API cleanups.

`image_plane_3d`'s default `width` is 1. Every other 3D primitive defaults to one world unit (a `shape_3d` has `size` `[1, 1, 1]`), so a plane with no params is as wide as a default box beside it. For a 16:9 source about a unit tall, set `"width": 1.6`.

`image_plane_3d` takes `opaque` (default false). A plane drawn blended, as every plane is by default, is ordered by its centre among the blended objects and doesn't write depth, so it can show in the wrong order against other blended objects it crosses. With `opaque` on, the plane is drawn with the opaque objects and writes depth, so it is ordered by depth against anything it intersects. Use it for a source with no transparent pixels. Alpha is ignored then, the colour's alpha, `opacity` and the source's: a transparent pixel shows its colour at full opacity, which is black for a fully transparent one, and a half-transparent one shows its colour undimmed. `opaque` casts a shadow over the whole rectangle, and has no effect on a `shadow_catcher`, which is always drawn as one.

```json
{
  "id": 2,
  "type": "image_plane_3d",
  "params": { "width": 2, "rotation": [0, 45, 0], "opaque": true }
}
```

A gradient given as a 3D colour, `color` on `shape_3d`, `image_plane_3d`, `text_3d` or `light_3d`, or `fog_color` on `render_3d`, fails `check` with `node.invalid_param` at that param's path, such as `/nodes/3/params/color`, instead of failing the render. These params' schema type is a color rather than a paint, so `NodeParamKind` has a `"color"` member and the generated TypeScript types of these params don't accept a gradient. No expression can produce a gradient, so nothing but a composition built in Rust rather than parsed from JSON can still reach the render-time "3D colours must be solid" error.
