---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`material_3d` projects its `texture` from a camera: wire a scene, usually a still `camera_3d`, into its new `projector` input and each point of any object it shades takes the texel that camera sees it at, so a still or a plate lands on rough geometry (shapes, extrusions, text, lathes and `instance_3d` copies) without UVs authored for one view, and the render camera can move around it. Faces turned away from the projector and points outside its view show the material's colour alone, and with `projector_occlusion`, on by default, so do points it can't see behind shadow casters, from a depth map drawn per projector as a light's shadow map is. `model_3d` takes a `projector` too, projecting its `texture` onto the material `texture_material` names.

```ts
const projected = n.material_3d({
  unlit: true,
  texture: n.media_in({ kind: "image", source: "street.jpg" }),
  projector: n.camera_3d({ position: [0, 1.2, 5], look_at: true, target: [0, 1, -1], fov: 50 }),
});
n.shape_3d({ size: [1.2, 1.2, 1.2], material: projected });
```
