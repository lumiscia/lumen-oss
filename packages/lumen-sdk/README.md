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

`output` picks what is rendered: an `mp4` (the default), a `png` or `jpeg` still, or a `png_sequence` or `jpeg_sequence`. `frames` is `start..end` of the timeline with `end` not included, like the player's `range: { in, out }`; an MP4 or sequence starts at time 0 and an MP4's audio is cut to the same span. A still is the frame at `start` (0 by default). A PNG keeps transparency, a JPEG is opaque and takes `jpegQuality` from 1 to 100 (default 90), and a sequence downloads as one ZIP of `frame_0000.png`-style files named with the timeline's frame numbers. `quality` and `codec` apply to `mp4` only. The downloaded response carries the right `Content-Type` and filename, and `outputFile(output)` gives the same content type and extension up front:

```ts
const clip = await lumen.render(composition, { output: { frames: { start: 30, end: 90 } } });
const poster = await lumen.render(composition, {
  output: { format: "png", frames: { start: 45 } },
});
const frames = await lumen.render(composition, {
  output: { format: "jpeg_sequence", frames: { end: 24 }, jpegQuality: 85 },
});
// import { outputFile } from "@lumiscia/lumen-sdk"
const { contentType, extension } = outputFile({ format: "jpeg_sequence" }); // "application/zip", "zip"
```

A range that is empty or past the timeline, or a sequence past the host's limits, is rejected with `invalid_output` or `output_too_large`.

A render on `lumen-server` can be stopped with `cancelRender`, which returns the render: `cancelled` if it was still queued, otherwise still `processing` until it stops at its next frame. `waitForRender` and `download` then throw a `LumenError` with the code `render_cancelled`, which is not retryable. A render whose output is already complete may still end `succeeded`. `cancelRender` is for `lumen-server`.

```ts
import { LumenError } from "@lumiscia/lumen-sdk";

const render = await lumen.render(composition);
await lumen.cancelRender(render.id);
try {
  await lumen.waitForRender(render);
} catch (error) {
  if (!(error instanceof LumenError && error.code === "render_cancelled")) throw error;
}
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

The [3D guide](https://github.com/lumiscia/lumen-oss/blob/main/docs/3d.md) covers the 3D scene nodes, `render_3d` and glTF models.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
