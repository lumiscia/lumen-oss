---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

3D lights can cast shadows. Turn on `cast_shadows` on a directional, spot or point `light_3d`, and objects between it and others block its light. `shadow_softness`, 0.02 by default, is how wide in world units shadow edges blur. A directional light's shadows cover what the camera sees of the scene, as far from the camera as its shadow map resolves edges as soft as `shadow_softness` (or 0.01, when sharper), so a floor running to the horizon keeps sharp shadows near the camera; a spot light's cover its cone, up to 170° across; and a point light's reach every direction. A `render_3d` draws shadows for the first 8 lights casting them in its scene, and the rest light the scene without shadows, named in a `scene.shadow_limit` warning. A scene with no light casting shadows renders exactly as before.

`shape_3d`, `image_plane_3d` and `model_3d` take `cast_shadows` and `receive_shadows`, both on by default. Unlit and glowing surfaces cast shadows too, so a shape standing in for a light, such as a bulb around a point light, needs `cast_shadows` off. Masked surfaces cast shadows only where they aren't cut away, blended ones where their alpha is at least 0.5, a model's in the shape of its current pose, and an image plane's in the shape of its raster's alpha.

On `shape_3d` and `image_plane_3d`, `shadow_catcher` shows nothing but the shadows falling on the surface: transparent black that darkens what the render is composited over by as much light as the shadows take away, times the colour's alpha, `opacity` and an image plane's raster's alpha, whether or not it is `unlit`. It shows none of its colours, hides what is behind it wherever that alpha isn't 0, and casts no shadows. That grounds a model on 2D footage or a flat background:

```json
{
  "nodes": [
    { "id": 1, "type": "background", "params": { "paint": [70, 130, 160, 255] } },
    {
      "id": 2,
      "type": "model_3d",
      "params": { "source": "asset:fox", "animation": "Walk", "scale": [0.02, 0.02, 0.02] }
    },
    {
      "id": 3,
      "type": "shape_3d",
      "params": { "shape": "plane", "size": [10, 1, 10], "shadow_catcher": true }
    },
    {
      "id": 4,
      "type": "light_3d",
      "params": {
        "light": "directional",
        "intensity": 3,
        "rotation": [-55, -40, 0],
        "cast_shadows": true,
        "shadow_softness": 0.04
      }
    },
    { "id": 5, "type": "light_3d", "params": { "light": "ambient", "intensity": 0.4 } },
    {
      "id": 6,
      "type": "camera_3d",
      "params": { "position": [0, 1.4, 4.2], "look_at": true, "target": [0, 0.7, 0] }
    },
    { "id": 7, "type": "merge_3d" },
    { "id": 8, "type": "render_3d" },
    { "id": 9, "type": "merge" },
    { "id": 10, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 2, "to_node": 7, "to_port": "objects" },
    { "from_node": 3, "to_node": 7, "to_port": "objects" },
    { "from_node": 4, "to_node": 7, "to_port": "objects" },
    { "from_node": 5, "to_node": 7, "to_port": "objects" },
    { "from_node": 6, "to_node": 7, "to_port": "objects" },
    { "from_node": 7, "to_node": 8, "to_port": "scene" },
    { "from_node": 1, "to_node": 9, "to_port": "base" },
    { "from_node": 8, "to_node": 9, "to_port": "overlay" },
    { "from_node": 9, "to_node": 10, "to_port": "source" }
  ]
}
```

The GPU recorder can draw into a depth target alone and sample it with a comparison sampler.
