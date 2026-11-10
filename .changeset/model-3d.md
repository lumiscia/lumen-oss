---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`model_3d` puts a glTF model in a 3D scene. Its `source` is a binary glTF (`.glb`) file, a path or `asset:<id>`, with its buffers and images packed inside it. It draws the model's default scene with its materials, and plays one of its animation clips: `animation` names the clip, by its name or its index, and empty plays none. `animation_speed` multiplies the clip's speed, `animation_offset` starts it that many seconds in, and `animation_loop`, on by default, plays it again from its start once it ends; off, the model holds the clip's last pose. `position`, `rotation` and `scale` place the model like any other scene node. A clip or material the model lacks fails the render with a message listing the ones it has. A model that plays no clip, or plays one at speed 0, is drawn once and kept across frames, like a still shape.

```json
{
  "nodes": [
    {
      "id": 1,
      "type": "model_3d",
      "params": {
        "source": "asset:fox",
        "animation": "Walk",
        "scale": [0.02, 0.02, 0.02],
        "rotation": [0, 90, 0]
      }
    },
    { "id": 2, "type": "render_3d" },
    { "id": 3, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 2, "to_port": "scene" },
    { "from_node": 2, "to_node": 3, "to_port": "source" }
  ]
}
```

A raster wired into `texture` replaces the material `texture_material` names, shown unlit with its colours as in 2D and sampled as `image_plane_3d` samples it. That puts a video on a television's screen:

```json
{
  "nodes": [
    { "id": 1, "type": "media_in", "params": { "kind": "video", "source": "asset:trailer" } },
    {
      "id": 2,
      "type": "model_3d",
      "params": { "source": "asset:television", "texture_material": "screen" }
    },
    { "id": 3, "type": "render_3d" },
    { "id": 4, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 2, "to_port": "texture" },
    { "from_node": 2, "to_node": 3, "to_port": "scene" },
    { "from_node": 3, "to_node": 4, "to_port": "source" }
  ]
}
```

What is read from the model:

- The default scene's node hierarchy, with each node's matrix or translation, rotation and scale.
- Triangle meshes: positions, normals (faces without them are lit flat), the first set of texture coordinates, and indices. Quantized attributes (`KHR_mesh_quantization`) are read too.
- Metallic-roughness materials: base colour, metallic-roughness, normal, emissive and occlusion textures, with the normal scale and occlusion strength; opaque, mask and blend alpha modes; double-sided materials; `KHR_materials_unlit` and `KHR_materials_emissive_strength`.
- PNG and JPEG textures, with their samplers' wrap and filter modes.
- Skins, and animation clips of node translation, rotation and scale with step, linear and cubic-spline interpolation.

What isn't:

- Files outside the `.glb`: a model that refers to one is rejected.
- Draco and meshopt compression, KTX2 and WebP textures, and `KHR_texture_transform`. A model that requires one of these, or any other extension not listed above, is rejected naming it; one that only uses it optionally loads without it.
- Morph targets, vertex colours, a second set of texture coordinates, and points and lines. A model with them loads and draws without them.
- The model's own lights and cameras.

The Lumen server takes `.glb` uploads as `model` assets, checking each by reading it as renders do. The browser preview has no models yet; a `model_3d` there fails, naming the source.
