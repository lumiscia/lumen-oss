# @lumiscia/lumen-preview

The framework-independent browser preview for Lumen: playback state, a transport, media decoding and audio. Use it to preview in plain JavaScript, or through `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte`. It needs a browser with WebGPU and `@lumiscia/lumen-bindings`.

```bash
pnpm add @lumiscia/lumen-preview @lumiscia/lumen-bindings @lumiscia/lumen-shared
```

```ts
import { createLumenPreview, mountPreview } from "@lumiscia/lumen-preview";
import { wasmUrl, workerUrl } from "@lumiscia/lumen-bindings/urls";
import { Composition } from "@lumiscia/lumen-shared";

const composition = new Composition({ timeline: { durationSeconds: 3 } });
const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
const title = composition.addNode({
  type: "text",
  params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
});
const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
composition.addNode({ type: "media_output", inputs: { source: merged } });

const preview = createLumenPreview();
mountPreview(document.body, { preview, composition, wasmUrl, workerUrl });

// Loading is asynchronous, and play() before it finishes is ignored.
document.querySelector("button")?.addEventListener("click", () => {
  if (preview.getSnapshot().isLoaded) preview.play();
});
```

Pass `media` and `fonts` (name to URL) when the composition uses them. `media` also takes the binary glTF (`.glb`) files `model_3d` nodes read, under the name in their `source`; they load with the composition, as images do. The preview renders in a worker; `@lumiscia/lumen-bindings/urls` names that worker and the wasm it loads, and a page that serves copies of them elsewhere passes those URLs instead. Either can be on another origin that sends CORS headers. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`. With Vite 6 or 7, the `@lumiscia/lumen-bindings` README has the dev server setting these URLs need.

`onError` is called with each failure the state's `error` shows. When the engine rejects the composition, its `diagnostics` say what is wrong and where:

```ts
mountPreview(document.body, {
  preview,
  composition,
  wasmUrl,
  workerUrl,
  onError: ({ code, message, diagnostics }) => {
    for (const { severity, code, path, message } of diagnostics) {
      console.warn(`${severity} ${code} at ${path || "/"}: ${message}`);
    }
  },
});
```

Diagnostic codes are public API, and new ones can appear in a patch release, so handle codes you don't know by their `severity`. The [engine README](https://github.com/lumiscia/lumen-oss/tree/main/crates/engine#diagnostics) lists them.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
