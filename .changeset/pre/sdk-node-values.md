---
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-sdk": minor
---

Compositions can be built as nested node values. `nodes` has one function per node kind, named after its `type`, that takes the node's params and input ports by name in one object and returns the `NodeInput` `addNode` takes, without adding anything. A port takes a node value as well as a node, and `composition.addOutput(source)` adds a `media_output` reading it. A value is added when a node reading it is, after its own inputs, so it gets the ids and JSON the same `addNode` calls would; the same object read twice is one node with two readers, and a value mutated into a cycle is an error. Ports keep their kinds: a scene value in a raster port, or a raster in `render_3d`'s `scene`, doesn't compile.

```ts
import { Composition, nodes as n } from "@lumiscia/lumen-sdk";

const scene = n.merge_3d({
  objects: [n.camera_3d({ position: [0, 1, 6] }), n.shape_3d({ shape: "torus" })],
});
const composition = new Composition().addOutput(
  n.raster_multimerge({
    layers: [
      n.render_3d({ scene, tone_mapping: "agx" }),
      n.glow({ glow_only: true, radius: 46, source: n.render_3d({ scene, exposure: -1 }) }),
    ],
  }),
);
```

`defineComposition`'s `build` may return a node value, and a `clip()` nests in one like any other node. `addNode` takes node values in its `inputs` too, and values may read nodes `addNode` returned.

`@lumiscia/lumen-types` adds `nodeInputPorts`, each kind's input ports with their kind and whether they take a list, and `NodeParamPortNameClash`, which is `never`: the generated types stop compiling if a kind ever gives a param and an input port the same name.

`addNode` itself still adds a node every time it is called. A failed `addNode` leaves the composition as it was, nested values included.
