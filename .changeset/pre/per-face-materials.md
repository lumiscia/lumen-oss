---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

A `shape_3d` box takes a material for each face, through `top_material`, `bottom_material`, `left_material`, `right_material`, `front_material` and `back_material`, and `text_3d` and `extrude_3d` take a `side_material` for their walls and bevel, leaving the caps to `material`. A face or side with no material wired uses `material`. They draw from the one mesh as an object for each run of faces sharing a material, so a voxel-style scene needs a shape per voxel instead of one per face, and under `instance_3d` every run is drawn from the same instance buffer. Face materials on a shape other than a box fail the render, naming the node and the input.

```ts
n.shape_3d({
  material: n.material_3d({ color: "#8a5a35" }),
  top_material: n.material_3d({ color: "#5fa04e" }),
});
```
