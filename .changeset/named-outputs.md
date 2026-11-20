---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

A node kind can name outputs beside its default `output`, and a connection reads one with `from_port`. `render_3d` names `color`, `depth` and `normal`: one `render_3d` draws every output something reads in one walk of the scene, so colour and depth no longer take two nodes, and a `from_port` the source doesn't have fails with `connection.invalid_from_port`, naming the outputs it has. An omitted `from_port` reads `output` as before. Definitions list each kind's outputs under `x-lumen-outputs`, and the SDK's `outputOf(node, name)` checks the name against the node's kind:

```ts
const render = n.render_3d({ scene, depth_near: 3, depth_far: 20 });
n.merge({ base: outputOf(render, "color"), mask: outputOf(render, "depth"), mask_mode: "luma" });
```
