# @lumiscia/lumen-types

Generated TypeScript types for Lumen composition JSON and node metadata. Use it to type compositions you read, store or send. It has no runtime dependencies.

```bash
pnpm add @lumiscia/lumen-types
```

```ts
import type { LumenComposition, NodeKind } from "@lumiscia/lumen-types";

declare const composition: LumenComposition;

const kinds: NodeKind[] = ["background", "text", "merge", "media_output"];
console.log(composition.nodes.length, kinds);
```

To build a composition, use [`@lumiscia/lumen-shared`](https://www.npmjs.com/package/@lumiscia/lumen-shared).

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
