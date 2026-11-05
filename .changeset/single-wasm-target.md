---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
---

`@lumiscia/lumen-bindings` ships one wasm build instead of four.

- `.` is now `/browser`, which works without a bundler and takes optional `previewWasmUrl` and `previewWorkerUrl` overrides. A pre-bundled preview worker ships at `/browser/worker.js`.
- `/bundler` stays as the Vite entry and needs no configuration. Remove `vite-plugin-wasm`, `vite-plugin-top-level-await` and any `optimizeDeps.exclude` for these packages.
- `/node` loads the same wasm with `initSync`. It parses compositions; rendering still needs a browser.
- `/web`, `/nodejs` and `/no-modules` are gone. Use `/browser`, `/node`, or `initSync` from `/browser`.
- `createLumenBindings` returns a plain object, and the `LumenBindings` class, its `target` field and `LumenBindingsLike` are gone. Non-URL wasm inputs go through `initSync`.
- `createLumenPreviewWorkerHost` no longer takes `previewWasmUrl`, because the session sends the worker the URL from its bindings.
