---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

`Player` no longer takes `bindings`: `<Player preview={preview} composition={composition} />` is enough. React and Svelte now depend on `@lumiscia/lumen-bindings` and start its preview worker and wasm from where they're installed, which Vite and plain ESM both resolve. A page that serves them elsewhere passes `workerUrl` and `wasmUrl` to `Player`.

This is breaking. `mountPreview` and `LumenPreviewSession` take `wasmUrl` and `workerUrl` in place of `bindings`, and `@lumiscia/lumen-bindings/urls` exports the defaults without loading the wasm. `LumenWorkerBindings` is gone, and `createLumenBindings` from `@lumiscia/lumen-bindings` no longer takes `previewWorkerUrl`; it only loads the wasm on the calling thread.
