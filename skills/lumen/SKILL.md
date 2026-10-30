---
name: lumen
description: Build, preview, and render videos programmatically with Lumiscia (hosted GPU rendering API) or Lumen (its open-source, self-hostable engine). Use when an app needs to generate MP4 video from code or JSON, when writing or editing Lumen composition JSON (nodes, connections, timeline, expressions, templates, audio), when using @lumiscia/lumen-sdk, @lumiscia/lumen-shared, @lumiscia/lumen-react, @lumiscia/lumen-svelte, @lumiscia/lumen-preview, or @lumiscia/lumen-bindings, when uploading media or creating renders against api.lumiscia.com, handling render webhooks, or running lumen-server locally. Not for contributing to the lumen-oss repository itself.
---

# Lumiscia and Lumen

Lumiscia renders video from JSON compositions on hosted GPUs. Lumen is the open-source engine it runs, which you can also self-host. Both take the same composition document.

## Mental model

- A **composition** is a node graph plus a timeline: `timeline` (`fps`, `duration_frames`), `render_settings` (`width`, `height`), `nodes`, and `connections`. Frames start at source nodes (`background`, `text`, `shape`, `media_in`, ...), flow through processing and compositing nodes, and end at exactly one `media_output`.
- Params can be literals or **expressions**: strings starting with `=` that are evaluated every frame (`"=progress(0, 30)"`). That is how you animate.
- Media is never inlined. Nodes name an **alias** (`"source": "hero"`), and each render request carries a `media` manifest mapping alias to `lumen:<media_id>`.
- **Preview** runs the same engine in the browser via WebAssembly (`@lumiscia/lumen-react`, `@lumiscia/lumen-svelte`, or `@lumiscia/lumen-preview` directly). It is free and needs no API key.
- **Render** happens server-side through `@lumiscia/lumen-sdk`: upload media, `render()`, then wait (WebSocket, webhook, or polling) and download the MP4.

## Quickstart (Node.js 22.18+ or Bun)

```bash
npm install @lumiscia/lumen-sdk @lumiscia/lumen-shared
export LUMISCIA_API_KEY="..."   # create one in the lumiscia.com dashboard
```

Use a **test API key** while wiring things up: test-key renders skip the GPU and media validation and complete with a sample artifact, so the whole flow runs for free. Switch to a live key once the integration works.

```ts title="render.mts"
import { writeFile } from "node:fs/promises";
import { Composition, Lumen } from "@lumiscia/lumen-sdk";
import { ease, progress, sec, tween } from "@lumiscia/lumen-shared";

const lumen = new Lumen({
  apiKey: process.env.LUMISCIA_API_KEY!,
  baseUrl: "https://api.lumiscia.com/v1", // the default; set explicitly so 0.3.x SDKs work too
});

const composition = new Composition({
  metadata: { name: "Title card" },
  renderSettings: { width: 1280, height: 720 },
  timeline: { fps: 30, durationSeconds: 4 },
});

const background = composition.addNode({
  type: "background",
  params: { paint: [18, 24, 32, 255] },
});
const title = composition.addNode({
  type: "text",
  params: {
    content: "Hello from Lumiscia",
    font_size: 96,
    font_weight: 700,
    color: [255, 255, 255, 255],
    opacity: progress(0, sec(1)),
    position: tween([120, 340], [120, 300], 0, sec(0.6), ease.outCubic),
  },
});
const stack = composition.addNode({ type: "raster_multimerge" });
const output = composition.addNode({ type: "media_output" });

composition
  .connect(background, stack, { toPort: "layers" })
  .connect(title, stack, { toPort: "layers" })
  .connect(stack, output, { toPort: "source" });

const { id, error } = await lumen.render(composition);
if (error || !id) {
  throw new Error(`${error?.code}: ${error?.message}`);
}

// Resolves on render.completed; rejects with a LumenRenderError (code, renderId, message) on failure.
await lumen.waitForRender(id, { onEvent: (event) => console.log(event.type) });

const artifact = await lumen.getRenderArtifact(id);
await writeFile("render.mp4", Buffer.from(await artifact.arrayBuffer()));
console.log("Saved render.mp4");
```

Run it with `node render.mts` or `bun render.mts`. `Composition` types every node `type` and param from the generated schema, so a wrong param name or enum value is a type error. `render()` also accepts plain composition JSON (`LumenComposition`).

## Media

Upload a file or ingest a public `https` URL, then map an alias to it in the render's `media` manifest. The `media_in` node references the alias, never the id or URL.

```ts
import { readFile } from "node:fs/promises";
import { mediaReference } from "@lumiscia/lumen-sdk";

const bytes = await readFile("hero.png");
const hero = await lumen.uploadMedia({
  body: bytes,
  contentType: "image/png",
  fileName: "hero.png",
  sizeBytes: bytes.byteLength,
});
// Or: const hero = await lumen.createUrlMedia({ url: "https://example.com/hero.png" });
if (hero.status === "rejected") {
  throw new Error(hero.rejectionReason ?? "Media was rejected");
}

const image = composition.addNode({
  type: "media_in",
  params: { kind: "image", source: "hero" }, // "video" also takes range_start/range_end, speed, loop_mode
});

const { id, error } = await lumen.render(composition, {
  media: { hero: mediaReference(hero) }, // "lumen:<media_id>"
});
```

Only media with `status: "validated"` renders. `"uploaded"` means the probe has not finished yet. Media expires 24 hours after creation, so upload shortly before rendering. For large files pass `multipart: true` (8 MiB parts). In Node, a `ReadableStream` body requires `multipart: true`. Audio uses the same manifest: the alias goes in an audio clip's `source_id` (or `AudioTrack.addClip({ sourceId })`).

## Waiting for a render

- **`waitForRender(id, { onEvent, signal })`** opens the render's WebSocket and resolves on `render.completed`. Good for scripts and workers.
- **Webhook**: pass `webhookUrl` (public `https`, not localhost) to `render()`. Lumiscia POSTs one `render.succeeded` or `render.failed` event (`id`, `status`, `artifactUrl`, `costMicros`, `error`, ...), retrying on non-2xx. Deliveries are signed with [Standard Webhooks](https://www.standardwebhooks.com) (`webhook-id`, `webhook-timestamp`, `webhook-signature`); verify them with `verifyWebhook` and your workspace's `whsec_...` signing secret:

```ts
import { LumenWebhookError, verifyWebhook } from "@lumiscia/lumen-sdk";

export async function handleLumisciaWebhook(request: Request): Promise<Response> {
  try {
    const event = await verifyWebhook({
      body: await request.text(), // the raw body; re-serialized JSON breaks the signature
      headers: request.headers,
      secret: process.env.LUMISCIA_WEBHOOK_SECRET!,
    });
    if (event.event === "render.succeeded") {
      const mp4 = await lumen.getRenderArtifact(event.id);
      // store mp4 and mark the job done; deliveries can repeat, so dedupe on event.id
    }
    return new Response(null, { status: 204 });
  } catch (error) {
    if (error instanceof LumenWebhookError) {
      return new Response(error.code, { status: 400 });
    }
    throw error;
  }
}
```

- **Polling**: `getRender(id)` returns `render.status` (`queued`, `processing`, `succeeded`, `failed`). Prefer the socket or a webhook over tight loops.

## Browser preview

Build the composition in the browser and preview it without uploads. Register local files under the same aliases the `media_in` nodes use. Send the composition JSON and files to your backend for the real render.

```tsx
import { LumenCanvas, createLumenPreview, useLumenPreview } from "@lumiscia/lumen-react";
import { createLumenBindings } from "@lumiscia/lumen-bindings/bundler";

const preview = createLumenPreview(); // create both once, outside components
const bindings = createLumenBindings();

export function Preview({ compositionJson, file }: { compositionJson: string; file: File }) {
  const state = useLumenPreview(preview); // frame, totalFrames, isLoaded, isPlaying, error
  return (
    <LumenCanvas
      preview={preview}
      bindings={bindings}
      compositionJson={compositionJson}
      mediaSources={[{ kind: "image", id: "hero", source: file }]}
    />
  );
}
```

Needs Vite with `vite-plugin-wasm` and `vite-plugin-top-level-await`. Control playback with `preview.play()`, `preview.pause()`, `preview.seek(frame)`. Svelte exposes the same `LumenCanvas` and `createLumenPreview` from `@lumiscia/lumen-svelte`.

## Self-hosted Lumen

`lumen-server` (in `crates/server` of github.com/lumiscia/lumen-oss) is a single-machine HTTP server with the same render payload. It needs a GPU (Linux + Vulkan or macOS + Metal). Point the same SDK at it:

```ts
const lumen = new Lumen({
  apiKey: process.env.LUMEN_SERVER_TOKEN ?? "local",
  baseUrl: "http://127.0.0.1:8080",
});
await lumen.render(composition, { media: { hero: "https://example.com/hero.png" } });
```

It has no media upload or URL ingest (put `https` URLs straight in the manifest), ignores `webhookUrl`, and keeps artifacts in memory. See https://docs.lumiscia.com/open-source/self-hosting.md.

## Composition JSON files

When you write composition JSON by hand, add the schema for editor and agent validation:

```json
{
  "$schema": "https://lumiscia.com/schema/composition.json",
  "timeline": { "fps": 30, "duration_frames": 90 },
  "render_settings": { "width": 1280, "height": 720 },
  "nodes": [
    { "id": 1, "type": "background", "params": { "paint": "#121820" } },
    { "id": 2, "type": "media_output" }
  ],
  "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
}
```

## Common mistakes

- **An old 0.3.x SDK.** If `verifyWebhook` or `LumenRenderError` aren't exported, the installed SDK predates the current API surface: its default `baseUrl` is the dashboard proxy (which rejects API keys), `waitForRender` rejects with `undefined` on failure, and `render.costCents` is always `undefined` (the API sends `costMicros`). Upgrade, or at least set `baseUrl: "https://api.lumiscia.com/v1"`.
- **Wrong `toPort` in `connect()`.** `toPort` is required. When you pass node objects (not bare ids), it is type-checked against the target's inputs: `source` (processing nodes, `media_output`, `clip`), `base`/`overlay`/`mask` (`merge`), `a`/`b` (`boolean`), or `layers` (`raster_multimerge`, `switch`).
- **Stacking several layers into `merge.overlay`.** `merge` takes one base and one overlay. Use `raster_multimerge` with `layers` (first connection at the bottom) for stacks.
- **No duration.** A `Composition` without a timeline is 1 frame long. Set `durationSeconds` or `duration_frames`.
- **URLs or media ids in `media_in.source`.** It must be a manifest alias. The hosted API rejects `http(s)://` URLs in nodes.
- **Exposing the API key.** Keep every SDK call on the server. Browser code only builds and previews.
- **Expecting `render()` to throw.** `render()` and `getRender()` return `{ error }`. Media methods and `getRenderArtifact()` throw a plain `{ code, message, details }` object, not an `Error`.
- **Fonts.** Renders bundle Roboto. A font loaded into the browser preview is not sent with the render, so other families can render differently.
- **Expired inputs.** Media expires after 24 hours and artifacts after 1 day (free) or 7 days (paid). Download artifacts promptly.
- **Limits.** Up to 60 fps, 10,800 frames, 3840×2160, 250 nodes, 1 MB of JSON, 50 manifest entries. Your plan may set lower limits.
- **Retries.** An identical composition + manifest + `webhookUrl` within 5 minutes returns the existing render with `cached: true`.

## References

- [references/composition.md](references/composition.md): document structure, ports, node catalog, templates (`clip`, `defineComposition`), and audio.
- [references/expressions.md](references/expressions.md): expression syntax, globals, animation functions, and the TypeScript helpers.

Docs (fetch the `.md` URL for clean markdown):

- Index of all pages: https://docs.lumiscia.com/llms.txt (everything in one file: https://docs.lumiscia.com/llms-full.txt)
- Composition format: https://docs.lumiscia.com/composition-format.md
- Expressions: https://docs.lumiscia.com/expressions.md and https://docs.lumiscia.com/guides/animation.md
- Builder: https://docs.lumiscia.com/guides/composition-builder.md
- Node reference: https://docs.lumiscia.com/nodes.md (one page per node, such as https://docs.lumiscia.com/nodes/source/text.md)
- Media: https://docs.lumiscia.com/guides/media-sources.md
- Node SDK: https://docs.lumiscia.com/sdks/node.md; Cloudflare Workers (import from `@lumiscia/lumen-sdk/cloudflare`): https://docs.lumiscia.com/sdks/cloudflare.md
- Preview: https://docs.lumiscia.com/sdks/react.md, https://docs.lumiscia.com/sdks/svelte.md, https://docs.lumiscia.com/guides/preview-to-render.md
- Webhooks: https://docs.lumiscia.com/guides/webhooks.md
- JSON Schema: https://lumiscia.com/schema/composition.json
