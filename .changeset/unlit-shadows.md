---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Add `unlit_shadows` to `material_3d`, so an unlit surface can darken where shadows fall on it. An unlit surface shows its base colour as is, multiplied by `1 - unlit_shadows × shadow`, where `shadow` is the fraction of the lights' light that shadows take away at that point, as a `shadow_catcher` measures it. It is 0 by default, which changes nothing, and at 1 a fully shadowed point is black. It reads the shadow maps lit surfaces already use, applies to the shadows of lights with `cast_shadows` on, and does nothing on lit surfaces or with `receive_shadows` off.

```json
{
  "id": 1,
  "type": "material_3d",
  "params": { "color": "#e0c080", "unlit": true, "unlit_shadows": 0.45 }
}
```
