---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

A node kind can name outputs beside its default `output`, and a connection reads one with `from_port`. `render_3d` names `color`, `depth` and `normal`: one `render_3d` draws every output something reads in one walk of the scene, so colour and depth no longer take two nodes, and a `from_port` the source doesn't have fails with `connection.invalid_from_port`, naming the outputs it has. An omitted `from_port` reads `output` as before. Definitions list each kind's outputs under `x-lumen-outputs`, and `@lumiscia/lumen-types` exports `nodeNamedOutputs`, each kind's outputs beside `output`. In the SDK, a node value or `addNode` reference of a kind that names outputs has an `out` holding each of them, typed so a port takes only an output of the kind it reads; `out` never reaches the composition's JSON:

```ts
const render = n.render_3d({ scene, depth_near: 3, depth_far: 20 });
n.merge({ base: render.out.color, mask: render.out.depth, mask_mode: "luma" });
const { normal } = composition.addNode(render).out;
```
