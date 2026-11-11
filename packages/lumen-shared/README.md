# @lumiscia/lumen-shared

The `Composition` builder for Lumen. You add nodes to an explicit node graph, wire them with typed `inputs`, and get composition JSON. It depends only on `@lumiscia/lumen-types`.

```bash
pnpm add @lumiscia/lumen-shared
```

```ts
import { Composition } from "@lumiscia/lumen-shared";

const composition = new Composition({ timeline: { durationSeconds: 3 } });
const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
const title = composition.addNode({
  type: "text",
  params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
});
const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
composition.addNode({ type: "media_output", inputs: { source: merged } });

console.log(JSON.stringify(composition));
```

The same graph can be written as node values. `nodes` has one function per node kind, named after its `type`, taking its params and input ports in one object. A value adds nothing until a node that reads it is added, here by `addOutput`, which adds a `media_output`. A value read by two nodes is one node with two readers.

```ts
import { Composition, nodes as n } from "@lumiscia/lumen-shared";

const composition = new Composition({ timeline: { durationSeconds: 3 } }).addOutput(
  n.merge({
    base: n.background({ paint: [24, 28, 36, 255] }),
    overlay: n.text({ content: "Hello, Lumen", font_size: 120, position: [96, 480] }),
  }),
);
```

Both produce the same JSON, and they mix: a node `addNode` returned can be a value's input, and a value can be one of `addNode`'s `inputs`. A `defineComposition` `build` can return a value too, and its `clip()` nests like any other node.

`@lumiscia/lumen-sdk` re-exports `Composition`, so you don't need both.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
