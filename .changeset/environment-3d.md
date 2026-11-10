---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

3D scenes can be lit by an HDR environment map. Set `environment_map` on `render_3d` to an equirectangular Radiance `.hdr` or OpenEXR `.exr` file, by path or as `asset:<id>`, and it replaces the built-in studio: surfaces take their diffuse light and their reflections from it, rough ones blurred as much as their roughness. `environment` stays its brightness and `environment_rotation` turns it about +Y in degrees. A scene lit by a map and no lights gets no default lights. The map's centre is the view along -Z, with +X to its right and +Y at its top.

`environment_background` shows the map behind the scene, through the exposure and tone mapping, and makes the render opaque; `environment_background_blur` blurs it from 0 (sharp) to 1. Shadow catchers darken the background by the light their shadows take away:

```json
{
  "nodes": [
    {
      "id": 1,
      "type": "model_3d",
      "params": { "source": "asset:fox", "scale": [0.02, 0.02, 0.02] }
    },
    {
      "id": 2,
      "type": "shape_3d",
      "params": { "shape": "plane", "size": [10, 1, 10], "shadow_catcher": true }
    },
    {
      "id": 3,
      "type": "light_3d",
      "params": {
        "light": "directional",
        "intensity": 2,
        "rotation": [-55, -40, 0],
        "cast_shadows": true
      }
    },
    {
      "id": 4,
      "type": "camera_3d",
      "params": { "position": [0, 1.4, 4.2], "look_at": true, "target": [0, 0.7, 0] }
    },
    { "id": 5, "type": "merge_3d" },
    {
      "id": 6,
      "type": "render_3d",
      "params": {
        "environment_map": "asset:studio",
        "environment_rotation": 90,
        "environment_background": true,
        "environment_background_blur": 0.3
      }
    },
    { "id": 7, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 5, "to_port": "objects" },
    { "from_node": 2, "to_node": 5, "to_port": "objects" },
    { "from_node": 3, "to_node": 5, "to_port": "objects" },
    { "from_node": 4, "to_node": 5, "to_port": "objects" },
    { "from_node": 5, "to_node": 6, "to_port": "scene" },
    { "from_node": 6, "to_node": 7, "to_port": "source" }
  ]
}
```

Each map is read and prefiltered once and kept while renders use it; turning it or changing its brightness doesn't read it again. Files over 256 MiB, maps over 8192 pixels a side or 8192 × 4096 in all, and multi-part, deep or mip-mapped EXR files are refused before they are decoded, and larger maps are shrunk to 4096 × 2048 as they are read. Non-finite and negative pixels read as black. The server accepts `.hdr` and `.exr` uploads as `environment` media with their `width` and `height`. The browser preview can't load environment maps yet, and a composition that uses one fails there with `media.not_found`.
