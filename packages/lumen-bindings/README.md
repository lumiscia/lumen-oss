# @lumiscia/lumen-bindings

The Lumen preview renderer compiled to WebAssembly, and the worker the preview runs it in. `@lumiscia/lumen-react` and `@lumiscia/lumen-svelte` depend on it and find both files themselves. Previewing needs a browser with WebGPU.

```bash
pnpm add @lumiscia/lumen-bindings @lumiscia/lumen-preview @lumiscia/lumen-shared
```

Entry points: `/urls` names the preview worker and its wasm without loading either, for `mountPreview`. `/browser` (also the default, and `/bundler`) and `/node` load the wasm on the calling thread to parse compositions and answer timeline questions, but can't render.

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

Both URLs are resolved with `new URL(..., import.meta.url)`, so Vite emits the files as assets without extra plugins, and plain ESM served from a static path or a CDN finds them next to `urls.js`. Another origin needs CORS headers on both files. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`. A page that serves copies elsewhere passes those URLs instead. In React or Svelte, use `Player` from `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte` instead of `mountPreview`.

With Vite 6 or 7, keep `@lumiscia/lumen-bindings` and `@lumiscia/lumen-preview` out of the dev server's dependency pre-bundling. It moves the modules that locate the worker, wasm and audio worker into `node_modules/.vite/deps`, so they 404. Vite 8 and `vite build` need nothing.

```js
// vite.config.js
export default defineConfig({
  optimizeDeps: { exclude: ["@lumiscia/lumen-bindings", "@lumiscia/lumen-preview"] },
});
```

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
