# @lumiscia/lumen-types

Generated TypeScript types for Lumen composition JSON and node metadata. Use it to type compositions you read, store or send. It has no runtime dependencies.

```bash
pnpm add @lumiscia/lumen-types
```

```ts
import type { LumenComposition, NodeKind } from "@lumiscia/lumen-types";

const outputs: NodeKind = "media_output";

function countOutputs(composition: LumenComposition): number {
  return composition.nodes.filter((node) => node.type === outputs).length;
}
```

`nodeSpecs` and `nodeSpecByKind` hold what the schema says about each node kind: its ports, params, defaults, enum options and limits.

```ts
import { nodeSpecByKind } from "@lumiscia/lumen-types";

for (const param of nodeSpecByKind.blur.params) {
  console.log(param.id, param.kind, param.defaultValue, param.constraints);
}
// radius float 4 { min: 0, step: 0.5 }
```

To build a composition, use [`@lumiscia/lumen-shared`](https://www.npmjs.com/package/@lumiscia/lumen-shared).

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
