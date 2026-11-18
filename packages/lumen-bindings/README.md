# @lumiscia/lumen-bindings

The Lumen preview renderer compiled to WebAssembly, and the files the preview runs on: the worker that runs it, and the audio worker and worklet. `@lumiscia/lumen-react` and `@lumiscia/lumen-svelte` depend on it and find the files themselves. Previewing needs a browser with WebGPU.

```bash
pnpm add @lumiscia/lumen-bindings @lumiscia/lumen-preview @lumiscia/lumen-shared
```

Entry points: `/urls` exports `runtime`, the manifest of the files the preview runs on, without loading any of them, for `mountPreview`. The default entry and `/node` load the wasm on the calling thread to parse compositions and answer timeline questions, but can't render.

```ts
import { createLumenPreview, mountPreview } from "@lumiscia/lumen-preview";
import { runtime } from "@lumiscia/lumen-bindings/urls";
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
mountPreview(document.body, { preview, composition, runtime });

// Loading is asynchronous, and play() before it finishes is ignored.
document.querySelector("button")?.addEventListener("click", () => {
  if (preview.getSnapshot().isLoaded) preview.play();
});
```

In React or Svelte, use `Player` from `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte` instead of `mountPreview`.

## Runtime files

Each entry of the manifest is `{ file, url }`. `url` is resolved with `new URL(..., import.meta.url)`, so Vite emits the files as assets without extra plugins, and plain ESM served from a static path or a CDN finds them next to `urls.js`. `file` is the file's name in a directory the page hosts itself.

| Entry          | `file` in `runtime`  | `file` in `runtime2d`   | Export                  |
| -------------- | -------------------- | ----------------------- | ----------------------- |
| `worker`       | `worker.js`          | `worker-2d.js`          | `/worker`, `/worker-2d` |
| `wasm`         | `lumen_wasm_bg.wasm` | `lumen_wasm_2d_bg.wasm` | `/wasm`, `/wasm-2d`     |
| `audioWorker`  | `audio-worker.js`    | `audio-worker.js`       | `/audio-worker`         |
| `audioWorklet` | `audio-worklet.js`   | `audio-worklet.js`      | `/audio-worklet`        |

To host them yourself, with any bundler or CDN and no dev server setting, copy the four files of one manifest into a directory and pass it as `runtimeUrl` with the manifest. Each file then loads as its `file` in that directory:

```ts
mountPreview(document.body, { preview, composition, runtime, runtimeUrl: "/lumen-runtime/" });
```

The exports in the table name each file for a bundler's asset imports, such as Vite's `?url`, or a copy step. A page that serves one file somewhere else passes a manifest of its own, such as `{ ...runtime, wasm: { file: "lumen_wasm_bg.wasm", url: wasmUrl } }`. Another origin needs CORS headers on the files. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`.

The package ships two builds, each a wasm and the worker that runs it. The full build, the default, renders 2D and 3D nodes and reads glTF models with Draco, meshopt and WebP compression, but not KTX2. The 2D build has no 3D and is about a quarter smaller to download; it reports each 3D node as a `node.unsupported` error. A page that only shows 2D compositions passes `runtime2d` from `/urls-2d` instead of `runtime` from `/urls`. A wasm runs only in its own build's worker. A page imports only the manifest it names, so a bundler emits only that build. `Player` from `@lumiscia/lumen-react/2d` and `@lumiscia/lumen-svelte/2d` uses the 2D build. The default and `/node` entry points load the full build.

```ts
import { runtime2d } from "@lumiscia/lumen-bindings/urls-2d";

mountPreview(document.body, { preview, composition, runtime: runtime2d });
```

With Vite 6 or 7, keep `@lumiscia/lumen-bindings` out of the dev server's dependency pre-bundling. It moves the manifest into `node_modules/.vite/deps`, so the files it locates 404. Vite 8 and `vite build` need nothing.

```js
// vite.config.js
export default defineConfig({
  optimizeDeps: { exclude: ["@lumiscia/lumen-bindings"] },
});
```

## Checking compositions

`check` finds what is wrong with a composition without loading or rendering it, from the default entry or `/node`. It returns diagnostics, each with a `severity`, a public `code`, a JSON pointer `path`, the `node` it's about when there is one, and a `message`. Given the font families the page provides, it also warns of text in any other family, which renders in the default font:

```ts
import { loadLumenBindings } from "@lumiscia/lumen-bindings/node";

const { check } = await loadLumenBindings();
const diagnostics = check(JSON.stringify(composition), ["Inter"]);
const ok = diagnostics.every((diagnostic) => diagnostic.severity !== "error");
```

Warnings never stop a composition from loading. Codes are public API, and new ones can appear in a patch release, so handle codes you don't know by their `severity`. The [engine README](https://github.com/lumiscia/lumen-oss/tree/main/crates/engine#diagnostics) lists them, along with the problems only rendering finds. A composition `loadComposition` rejects throws an `Error` with the same `diagnostics`.

`mediaDependencies` lists what a composition needs before it renders, as the engine finds it: `{ media, dynamic, fonts }`. Each `media` entry is a source as written, the kinds it is read as, and where it is used, in a node of the root or of a template or in an audio clip. A source an expression computes is in `dynamic`, and the font families text names are in `fonts`. It throws, with `diagnostics`, on a composition the engine rejects. The result is typed as `MediaInventory` in `@lumiscia/lumen-types`:

```ts
const { mediaDependencies } = await loadLumenBindings();
for (const { source, kinds, uses } of mediaDependencies(json).media) {
  console.log(source, kinds, uses.length);
}
```

`evaluate` gives the value of one param of a root node at a frame, as a render would evaluate it and without rendering, so an easing curve, a timing or a `table()` lookup can be read without reading pixels. It takes the composition as JSON text or an object and a request `{ node, param, frame, instance? }`, and returns the value in the JSON that sets the param. A flattened param such as `position` is addressed by its own name, and `instance` picks the copy of an `instance_3d` whose `position`, `rotation` and `scale` are evaluated. It throws, with `diagnostics`, on a composition the engine rejects, a node or param that doesn't exist, a node inside a template, and an expression that fails, which includes one that needs media. Text is measured in the bundled font. The request is typed as `EvaluateRequest` in `@lumiscia/lumen-types`:

```ts
const { evaluate } = await loadLumenBindings();
const position = evaluate(composition, { node: 12, param: "position", frame: 90 });
// [1.5, 0, -3]
```

`modelInfo` lists the animation clips, materials and cameras of a binary glTF (`.glb`), which a `model_3d` selects with `animation`, `texture_material` and `camera`. It reads the file as a render would and throws on one it can't read. An unnamed clip, material or camera has no `name`; a clip or camera is then selected by its `index`, as text:

```ts
const { modelInfo } = await loadLumenBindings();
const { animations, materials, cameras } = modelInfo(new Uint8Array(await file.arrayBuffer()));
// animations: [{ index: 0, name: "Walk", duration: 0.7 }]
// materials: [{ index: 0, name: "screen", hasBaseColorTexture: false }]
// cameras: [{ index: 0, name: "Shot" }]
```

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
