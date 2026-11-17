---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Move the material params off `shape_3d`, `text_3d` and `image_plane_3d` onto a new `material_3d` node, which an object takes through a `material` input. `color`, `metallic`, `roughness`, `emissive`, `opacity`, `unlit`, `double_sided`, `cast_shadows`, `receive_shadows` and `shadow_catcher` are now params of `material_3d`, and those nodes no longer accept them. One `material_3d` can be wired into any number of objects, which then share its params, and every param still takes an expression. This breaks compositions that set those params on an object.

An object with no `material_3d` wired in renders as before: `shape_3d` and `text_3d` are lit, single-sided and white, and `image_plane_3d` is unlit and double-sided. A wired `material_3d` applies exactly as it is set, so a default `material_3d` makes an `image_plane_3d` lit and single-sided; set `unlit` and `double_sided` to keep it as it was. `model_3d` keeps the materials of its glTF file.

Before:

```json
{
  "nodes": [
    {
      "id": 1,
      "type": "shape_3d",
      "params": { "color": "#e6b45a", "metallic": 1, "roughness": 0.2 }
    },
    {
      "id": 2,
      "type": "shape_3d",
      "params": { "color": "#e6b45a", "metallic": 1, "roughness": 0.2, "position": [2, 0, 0] }
    }
  ]
}
```

After:

```json
{
  "nodes": [
    {
      "id": 0,
      "type": "material_3d",
      "params": { "color": "#e6b45a", "metallic": 1, "roughness": 0.2 }
    },
    { "id": 1, "type": "shape_3d" },
    { "id": 2, "type": "shape_3d", "params": { "position": [2, 0, 0] } }
  ],
  "connections": [
    { "from_node": 0, "to_node": 1, "to_port": "material" },
    { "from_node": 0, "to_node": 2, "to_port": "material" }
  ]
}
```

The TypeScript SDK takes the material as a nested node: `n.shape_3d({ material: n.material_3d({ color: "#e6b45a" }) })`.
