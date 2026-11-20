---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`material_3d` gets an `alpha_mode` of `mask`, with an `alpha_cutoff`, so a cut-out card can be an `image_plane_3d` instead of a glTF model. A masked surface draws with the opaque objects and writes depth, so planes that cross draw in the right order, discards the texels whose alpha is below the cutoff and shows the rest at full opacity. It casts shadows shaped by the cut-out, as a glTF `MASK` material does, and works on every object that takes a material. `shadow_catcher` can't be masked, and a document that sets both fails `check`.

```json
{
  "id": 3,
  "type": "material_3d",
  "params": { "unlit": true, "double_sided": true, "alpha_mode": "mask" }
}
```
