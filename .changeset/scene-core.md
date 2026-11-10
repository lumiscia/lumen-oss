---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

3D scenes, in a new `scene` node category. `shape_3d` makes a box, sphere, plane, cylinder, torus or cone; `camera_3d` is a perspective or orthographic viewpoint that can aim at a `target`; `light_3d` is a directional, point, spot or ambient light; and `merge_3d` groups any of them under one transform. Each outputs a scene, which only scene inputs take. `render_3d` draws the scene wired to its `scene` input into a raster with a transparent background, so it composites like any other layer: metallic-roughness lighting from the scene's lights and a built-in studio environment whose brightness is `environment`, opaque objects hiding what is behind them and blended ones (`opacity` below 1) drawn over them from back to front, with `exposure`, `tone_mapping` (`none`, `aces` or `agx`), fog, and `supersample` for antialiasing. A scene without lights gets a default key light and ambient fill, and one without a camera is seen from 5 units along +Z.

World units are metres, Y is up and cameras look down -Z. `position`, `rotation` and `scale` are `[x, y, z]` values, so one expression can move all three axes, and a shape's `size` is its width, height and depth. `rotation` is in degrees, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z. A light's `intensity` is a unitless multiplier.

```json
{
  "nodes": [
    {
      "id": 1,
      "type": "shape_3d",
      "params": {
        "shape": "torus",
        "rotation": "=vec3(70, frame * 3, 0)",
        "color": "#e6b45a",
        "metallic": 1,
        "roughness": 0.25
      }
    },
    { "id": 2, "type": "camera_3d", "params": { "position": [0, 1.5, 4], "look_at": true } },
    { "id": 3, "type": "merge_3d" },
    { "id": 4, "type": "render_3d", "params": { "tone_mapping": "agx" } },
    { "id": 5, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 3, "to_port": "objects" },
    { "from_node": 2, "to_node": 3, "to_port": "objects" },
    { "from_node": 3, "to_node": 4, "to_port": "scene" },
    { "from_node": 4, "to_node": 5, "to_port": "source" }
  ]
}
```

A `render_3d` whose scene has no expressions is kept across frames like any other still layer. Wiring a scene into a raster input fails `connection.wrong_kind`, whose message says to draw it through a `render_3d`. Rust embedders get the rendering behind the engine's new `3d` feature, which `lumen-server` and the WebAssembly bindings turn on; without it, a composition with any scene node fails `check` with the new `node.unsupported` code. The maths and meshes that need no GPU are in the new `lumen-engine-3d` crate.
