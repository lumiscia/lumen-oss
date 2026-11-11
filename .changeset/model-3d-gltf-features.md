---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-sdk": minor
---

`model_3d` draws more of glTF:

- **Vertex colours** (`COLOR_0`, RGB or RGBA, float or normalised 8- or 16-bit) multiply the base colour in linear light, as its factor and texture do, lit or unlit, and their alpha blends or masks with it.
- **A second set of texture coordinates** (`TEXCOORD_1`): each map reads the set its `texCoord` names. A map naming a third set is left out.
- **`KHR_texture_transform`** on every map: offset, rotation and scale, and its own `texCoord`, which overrides the map's. Rotation turns as the Khronos sample renderer and three.js do.
- **Morph targets**: position and normal displacements, blended on the GPU by the mesh's or node's default weights and by clips animating `weights`, before skinning. Tangent displacements are ignored. A mesh may have up to 256 targets, and a model up to 4,000,000 displacements (one for each vertex of each target) and 1,048,576 weights; a scene draws up to 1,048,576 weights in a frame. A weight may be at most 10,000 either way (a clip sample overshooting it is held to it), and a vertex its targets would move past what a float holds at that weight is rejected. Shadows hold a morphed mesh at whatever weights it is drawn with. Morphed normals are unit length before skinning, falling back to the unmorphed normal where targets cancel it, and every vertex normal is normalised before it is interpolated. Draco-compressed primitives with morph targets are rejected. A model with still weights is kept across frames like any still model, and only the weights are uploaded each frame while a clip plays them. A clip's `duration` counts its weight channels.
- **The model's own lights and cameras**, opt-in. `lights: true` adds its `KHR_lights_punctual` lights where its nodes place them. Their units are `light_3d`'s: a directional light's lux and a point or spot light's candela are used as they are, so a point light of intensity `I` lights a surface `d` metres away as a directional light of `I / d²` does, and a spot light fades from its inner cone to its outer one. A light whose colour times intensity overflows is rejected. They cast no shadows. `camera` views the scene from one of the model's cameras, by name or else index, placed by the first node that places it without that node's scale; it counts as a `camera_3d` at the model, so the first camera in the scene still wins, and the output's aspect ratio is used. `modelInfo` and an uploaded model's `metadata` list the cameras (`index`, and `name` when named).

Points, lines, and triangle strips and fans are still not drawn, and now raise a `scene.model_primitives_skipped` warning naming the model and how many were left out.

Nothing in a composition breaks, but models now look as their files describe: vertex colours tint them, maps naming `TEXCOORD_1` read it instead of the first set, and morph targets move them. Files requiring `KHR_texture_transform` or `KHR_lights_punctual` are no longer rejected. `LumenModelInfo` gains `cameras`, and the Rust `ModelInfo` a `cameras` field.

```json
{
  "id": 1,
  "type": "model_3d",
  "params": { "source": "asset:robot", "animation": "Talk", "lights": true, "camera": "Closeup" }
}
```

```ts
import { nodes } from "@lumiscia/lumen-shared";

const robot = nodes.model_3d({
  source: "asset:robot",
  animation: "Talk",
  lights: true,
  camera: "Closeup",
});
const { cameras } = modelInfo(bytes); // [{ index: 0, name: "Closeup" }]
```
