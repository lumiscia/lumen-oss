---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`render_3d` can output its scene's depth or view-space normals as a raster for 2D effects such as fog, depth of field or relighting. Its new `output` param is `"color"` (the default, which renders as before), `"depth"` or `"normal"`. A node still has one output: a composition that wants colour and depth wires the same scene into two `render_3d` nodes, and the second shares the models, meshes and image-plane sources the first loaded.

- `depth` is the distance along the camera's view, mapped from 0 at `depth_near` to 1 at `depth_far` (each 0 by default, which uses the camera's `near` or `far`), as opaque grey, and 1 where nothing is drawn. Rasters hold half floats, finest near 0, so set the range around the part of the scene the effect needs.
- `normal` is the shaded normal (after normal maps, morph targets and skinning, reversed on the back of a double-sided surface) in the camera's space, +X right, +Y up, +Z towards the camera, stored as `n * 0.5 + 0.5`, opaque, and transparent where nothing is drawn.

Both are stored as data, with no transfer function (an 8-bit output encodes them as it does light, so a depth of 0.5 is code 188), skip lighting, shadows and the environment, and ignore `supersample`: each pixel is the nearest surface at its centre. Opaque surfaces write them, masked ones where they reach their cutoff, and blended ones, such as image planes, where their alpha reaches half.

Fog with existing nodes, `merge` with a luma mask:

```json
{ "id": 10, "type": "render_3d" },
{ "id": 11, "type": "render_3d", "params": { "output": "depth", "depth_near": 3, "depth_far": 14 } },
{ "id": 12, "type": "background", "params": { "paint": [205, 215, 228, 255] } },
{ "id": 13, "type": "merge", "params": { "mask_mode": "luma" } }
```

with the scene wired to both `render_3d` nodes, `10` to the merge's `base`, `12` to its `overlay` and `11` to its `mask`.
