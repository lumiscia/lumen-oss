---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Add a `texture` input to `material_3d`, so a `shape_3d` can show a raster with real depth, fog and shadows, such as a pixel-art planet. The texture multiplies `color` and is lit like the rest of the surface. `sampling` reads it as `image_plane_3d` reads its source: `linear` by default, or `nearest` for pixel art. Primitives now have texture coordinates. A sphere is wrapped equirectangularly from −Z, so a camera on +Z sees the texture's middle. A box shows the whole texture on each face, and a plane shows it seen from above. A cylinder, cone or torus is wrapped around Y. The `shape_3d` docs give the exact layout. A textured material wired into `text_3d`, `extrude_3d`, `lathe_3d` or `image_plane_3d` fails the render, naming the node.

```json
{
  "nodes": [
    { "id": 0, "type": "media_in", "params": { "kind": "image", "source": "planet.png" } },
    { "id": 1, "type": "material_3d", "params": { "sampling": "nearest" } },
    { "id": 2, "type": "shape_3d", "params": { "shape": "sphere", "size": [2, 2, 2] } },
    { "id": 3, "type": "render_3d" }
  ],
  "connections": [
    { "from_node": 0, "to_node": 1, "to_port": "texture" },
    { "from_node": 1, "to_node": 2, "to_port": "material" },
    { "from_node": 2, "to_node": 3, "to_port": "scene" }
  ]
}
```
