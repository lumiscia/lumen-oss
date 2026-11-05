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

`@lumiscia/lumen-sdk` re-exports `Composition`, so you don't need both.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
