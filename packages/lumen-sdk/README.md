# @lumiscia/lumen-sdk

TypeScript client for the hosted [Lumiscia](https://lumiscia.com) API. Build a composition, upload media, render it, wait for the result and download it. It re-exports the `Composition` builder.

```bash
pnpm add @lumiscia/lumen-sdk
```

```ts
import { Composition, Lumen } from "@lumiscia/lumen-sdk";

const composition = new Composition({ timeline: { durationSeconds: 3 } });
const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
const title = composition.addNode({
  type: "text",
  params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
});
const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
composition.addNode({ type: "media_output", inputs: { source: merged } });

const lumen = new Lumen({ apiKey: process.env.LUMISCIA_API_KEY! });
const render = await lumen.render(composition);
console.log((await lumen.waitForRender(render)).url);
```

The graph can also be written as nested node values, with the same node names and ports and the same JSON. `nodes` has one function per node kind; `addOutput` adds the tree and its `media_output`:

```ts
import { Composition, nodes as n } from "@lumiscia/lumen-sdk";

const composition = new Composition({ timeline: { durationSeconds: 3 } }).addOutput(
  n.merge({
    base: n.background({ paint: [24, 28, 36, 255] }),
    overlay: n.text({ content: "Hello, Lumen", font_size: 120, position: [96, 480] }),
  }),
);
```

`render` takes `quality`, a preset name (`draft`, `standard`, `high` or `lossless` on every host), and `codec`, `h264`, `hevc` or `av1`. They win over the composition's `renderSettings.quality` and `renderSettings.codec`, and a host that can't serve them rejects the render:

```ts
const composition = new Composition({ renderSettings: { codec: "hevc", quality: "high" } });
await lumen.render(composition, { quality: "lossless", codec: "h264" });
```

`examples/node` also uploads a clip with `lumen.uploadMedia` and passes it to `render` as `media`.

Failed requests throw a `LumenError` with a `code` and `status`. When a render is rejected because the composition is invalid, its `diagnostics` say what is wrong and where:

```ts
import { LumenError } from "@lumiscia/lumen-sdk";

try {
  await lumen.render(composition);
} catch (error) {
  if (!(error instanceof LumenError)) throw error;
  for (const { severity, code, path, message } of error.diagnostics) {
    console.error(`${severity} ${code} at ${path || "/"}: ${message}`);
  }
}
```

`diagnostics` is empty for every other failure. Diagnostic codes are public API, and new ones can appear in a patch release, so handle codes you don't know by their `severity`. The [engine README](https://github.com/lumiscia/lumen-oss/tree/main/crates/engine#diagnostics) lists them.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
