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

Both URLs are resolved with `new URL(..., import.meta.url)`, so Vite emits the files as assets without extra plugins, and plain ESM served from a static path or a CDN finds them next to `urls.js`. Another origin needs CORS headers on both files. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`. A page that serves copies elsewhere passes those URLs instead. `@lumiscia/lumen-bindings/worker` and `@lumiscia/lumen-bindings/wasm` name the two files for a bundler's asset imports, such as Vite's `?url`. In React or Svelte, use `Player` from `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte` instead of `mountPreview`.

With Vite 6 or 7, keep `@lumiscia/lumen-bindings` and `@lumiscia/lumen-preview` out of the dev server's dependency pre-bundling. It moves the modules that locate the worker, wasm and audio worker into `node_modules/.vite/deps`, so they 404. Vite 8 and `vite build` need nothing.

```js
// vite.config.js
export default defineConfig({
  optimizeDeps: { exclude: ["@lumiscia/lumen-bindings", "@lumiscia/lumen-preview"] },
});
```

`check` finds what is wrong with a composition without loading or rendering it, on any entry point but `/urls`. It returns diagnostics, each with a `severity`, a public `code`, a JSON pointer `path`, the `node` it's about when there is one, and a `message`. Given the font families the page provides, it also warns of text in any other family, which renders in the default font:

```ts
import { createLumenBindings } from "@lumiscia/lumen-bindings/node";

const { check } = await createLumenBindings().preview();
const diagnostics = check(JSON.stringify(composition), ["Inter"]);
const ok = diagnostics.every((diagnostic) => diagnostic.severity !== "error");
```

Warnings never stop a composition from loading. Codes are public API, and new ones can appear in a patch release, so handle codes you don't know by their `severity`. The [engine README](https://github.com/lumiscia/lumen-oss/tree/main/crates/engine#diagnostics) lists them, along with the problems only rendering finds. A composition `loadComposition` rejects throws an `Error` with the same `diagnostics`.

`mediaDependencies` lists what a composition needs before it renders, as the engine finds it: `{ media, dynamic, fonts }`. Each `media` entry is a source as written, the kinds it is read as, and where it is used, in a node of the root or of a template or in an audio clip. A source an expression computes is in `dynamic`, and the font families text names are in `fonts`. It throws, with `diagnostics`, on a composition the engine rejects. The result is typed as `MediaInventory` in `@lumiscia/lumen-types`:

```ts
const { mediaDependencies } = await createLumenBindings().preview();
for (const { source, kinds, uses } of mediaDependencies(json).media) {
  console.log(source, kinds, uses.length);
}
```

`modelInfo` lists the animation clips and materials of a binary glTF (`.glb`), which a `model_3d` selects with `animation` and `texture_material`. It reads the file as a render would and throws on one it can't read. An unnamed clip or material has no `name`; a clip is then selected by its `index`, as text:

```ts
const { modelInfo } = await createLumenBindings().preview();
const { animations, materials } = modelInfo(new Uint8Array(await file.arrayBuffer()));
// animations: [{ index: 0, name: "Walk", duration: 0.7 }]
// materials: [{ index: 0, name: "screen", hasBaseColorTexture: false }]
```

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
