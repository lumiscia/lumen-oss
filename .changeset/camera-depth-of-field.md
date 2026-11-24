---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`camera_3d` has a lens for depth of field. With an `f_stop` above 0, what is nearer or farther than `focus_distance` blurs as through a full-frame camera with the same `fov`, and `aperture_blades` and `aperture_rotation` shape the bokeh. `render_3d` draws the scene from `dof_samples` places across the lens (16 by default, up to 64) and averages them, so a blurred near edge turns see-through and shows what is behind it, and mattes blur with the colour. An `f_stop` of 0, the default, draws as before.

```json
{
  "id": 7,
  "type": "camera_3d",
  "params": { "fov": 16, "f_stop": 1.4, "focus_distance": 6, "aperture_blades": 6 }
}
```
