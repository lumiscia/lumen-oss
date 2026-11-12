---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`image_plane_3d` shows a raster on a flat rectangle in a 3D scene, so text, video, an image or any other 2D node can sit in depth under a real camera. Its `source` input takes the raster, and it outputs a scene like `shape_3d`, with the same `position`, `rotation`, `scale` and material params. A video or image works the same way: wire a `media_in`, or a `resize` of one, into `source`. `width` is in world units and the height follows the source's aspect ratio, taken from its full display size; a `clip` keeps the canvas its content declares, padding or cropping the content to it, rather than the composition's.

A plane is `unlit` and `double_sided` by default, so one that faces the camera shows its source's colours as in 2D, to within rounding, whatever `render_3d`'s `exposure` and `tone_mapping`. Set `unlit` to false to light it, with the raster as its base colour; `color` multiplies the raster, and `emissive` scales with it. A plane is drawn blended unless `opaque` is on, back to front with other blended objects, so its transparent texels show what is behind it and never hide it, and `opacity` multiplies the source's alpha. Seen from behind, the raster is mirrored. Planes are ordered by their centres, so planes that intersect, or overlap while crossing in depth, can show in the wrong order; layers separated in depth, and opaque shapes in front of or behind planes, are ordered correctly. A source keeps its detail when it is shown at its own size or larger, where `render_3d`'s `supersample` only antialiases the plane's edges; shown smaller, it is filtered with mipmaps before `supersample` averages it.

```json
{
  "nodes": [
    {
      "id": 1,
      "type": "text",
      "params": {
        "content": "Lumen",
        "font_family": "Inter",
        "font_size": 160,
        "position": [420, 280]
      }
    },
    {
      "id": 2,
      "type": "image_plane_3d",
      "params": { "width": 4, "position": [0, 0, -1], "rotation": [0, 25, 0] }
    },
    { "id": 3, "type": "render_3d" },
    { "id": 4, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 2, "to_port": "source" },
    { "from_node": 2, "to_node": 3, "to_port": "scene" },
    { "from_node": 3, "to_node": 4, "to_port": "source" }
  ]
}
```

Wiring a raster into a scene input now fails `connection.wrong_kind` with a message that names `image_plane_3d`.
