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

To build a composition, use [`@lumiscia/lumen-shared`](https://www.npmjs.com/package/@lumiscia/lumen-shared).

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
