---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

The browser preview's runtime files (the render worker, the wasm, the audio worker and the audio worklet) now all come from `@lumiscia/lumen-bindings`, which describes each build with one manifest: `runtime` from `@lumiscia/lumen-bindings/urls` and `runtime2d` from `@lumiscia/lumen-bindings/urls-2d`. Each entry is `{ file, url }`: where the file is served by default, and its name in a directory you host yourself.

`<Player preview={preview} composition={composition} />` in React and Svelte works as before, with no configuration.

`mountPreview` and `LumenPreviewSession` take the manifest as `runtime`, in place of `wasmUrl` and `workerUrl`:

```ts
// before
import { wasmUrl, workerUrl } from "@lumiscia/lumen-bindings/urls";
mountPreview(host, { preview, composition, wasmUrl, workerUrl });

// after
import { runtime } from "@lumiscia/lumen-bindings/urls";
mountPreview(host, { preview, composition, runtime });
```

Self-hosting with `runtimeUrl` now takes every file from one package, and selects the 2D build when given `runtime2d`. Copy the four files the manifest names from `@lumiscia/lumen-bindings` into one directory; `audio-worker.js` and `audio-worklet.js` no longer come from `@lumiscia/lumen-preview`:

```ts
// before: worker.js and lumen_wasm_bg.wasm from lumen-bindings, the audio files from lumen-preview,
// and the 2D build only by passing wasmUrl and workerUrl as well
mountPreview(host, { preview, composition, runtimeUrl: "/lumen-runtime/" });

// after: worker-2d.js, lumen_wasm_2d_bg.wasm, audio-worker.js and audio-worklet.js, all from lumen-bindings
import { runtime2d } from "@lumiscia/lumen-bindings/urls-2d";
mountPreview(host, { preview, composition, runtime: runtime2d, runtimeUrl: "/lumen-runtime/" });
```

To serve one file somewhere else, pass a manifest of your own. On `Player`, `runtime` replaces the `wasmUrl` and `workerUrl` props and defaults to the manifest of the entry point it comes from:

```tsx
// before
<Player preview={preview} composition={composition} wasmUrl={wasmUrl} workerUrl={workerUrl} />;

// after
import { runtime } from "@lumiscia/lumen-bindings/urls";
<Player
  preview={preview}
  composition={composition}
  runtime={{ ...runtime, wasm: { file: "lumen_wasm_bg.wasm", url: wasmUrl } }}
/>;
```

`audio.workerModuleUrl` and `audio.workletModuleUrl` are gone from the session's options (use `runtime.audioWorker` and `runtime.audioWorklet`), and so is `runtimeFileUrl`. The Vite 6 and 7 dev setting now needs only `optimizeDeps: { exclude: ["@lumiscia/lumen-bindings"] }`.

`@lumiscia/lumen-bindings` has one export path per module or file: `.`, `./node`, `./urls`, `./urls-2d`, `./wasm`, `./wasm-2d`, `./worker`, `./worker-2d`, `./audio-worker` and `./audio-worklet`. Import `@lumiscia/lumen-bindings` instead of `/browser` or `/bundler`, and `@lumiscia/lumen-bindings/worker` instead of `/browser/worker.js`.

`createLumenBindings(options)` is now `loadLumenBindings(options)`, which returns a promise of the bindings themselves, and the `LumenBindings` type is gone:

```ts
// before
const { check } = await createLumenBindings().preview();

// after
import { loadLumenBindings } from "@lumiscia/lumen-bindings/node";
const { check } = await loadLumenBindings();
```
