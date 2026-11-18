---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

`wgsl_shader` takes an optional `camera` scene input, so a shader can line up with a 3D scene without rebuilding the camera from uniforms. The shader reads the scene's camera, chosen as `render_3d` chooses it, as `lumen.view` (world to view), `lumen.projection` (view to clip, for the shader's output aspect ratio) and `lumen.camera_position` (world space). A scene without a camera gives `render_3d`'s default camera, and a shader without a `camera` input sees the matrices as the identity and the position as zero, so existing shaders keep working. Scene inputs need the `3d` feature.

```json
[
  { "id": 5, "type": "camera_3d", "params": { "position": [0, 2, 8], "look_at": true } },
  {
    "id": 6,
    "type": "wgsl_shader",
    "params": { "shader": "…uses lumen.projection * lumen.view…" },
    "in": { "source": [4], "camera": 5 }
  }
]
```
