# @lumiscia/lumen-react

## 0.5.1

### Patch Changes

- Updated dependencies [bbab23f]
- Updated dependencies [d14deff]
- Updated dependencies [14af846]
- Updated dependencies [6f5b681]
- Updated dependencies [6285ab0]
- Updated dependencies [6a6ee44]
- Updated dependencies [10b9793]
- Updated dependencies [87164d8]
- Updated dependencies [5406425]
- Updated dependencies [7fcff92]
- Updated dependencies [ee8b9bf]
- Updated dependencies [f9952fa]
- Updated dependencies [9cd9f55]
  - @lumiscia/lumen-bindings@0.5.1
  - @lumiscia/lumen-preview@0.5.1

## 0.5.0

### Minor Changes

- 4df3509: Diagnostics reach the TypeScript packages. The wasm bindings add `check(json, fonts?)`, which returns what is wrong with a composition without loading it, and a composition `loadComposition` rejects throws with the same `diagnostics`. `LumenError` in the SDK has `diagnostics`, filled when a render is rejected for an invalid composition. The preview and both `Player` components take `onError`, which receives each failure with its diagnostics. `LumenDiagnostic` is exported from `@lumiscia/lumen-types`.
- 29f9e9b: `Player` no longer takes `bindings`: `<Player preview={preview} composition={composition} />` is enough. React and Svelte now depend on `@lumiscia/lumen-bindings` and start its preview worker and wasm from where they're installed, which Vite and plain ESM both resolve. A page that serves them elsewhere passes `workerUrl` and `wasmUrl` to `Player`.

  This is breaking. `mountPreview` and `LumenPreviewSession` take `wasmUrl` and `workerUrl` in place of `bindings`, and `@lumiscia/lumen-bindings/urls` exports the defaults without loading the wasm. `LumenWorkerBindings` is gone, and `createLumenBindings` from `@lumiscia/lumen-bindings` no longer takes `previewWorkerUrl`; it only loads the wasm on the calling thread.

### Patch Changes

- 92b28e3: Every package now has a README, so its npm page describes it.
- Updated dependencies [6cddffa]
- Updated dependencies [db420ff]
- Updated dependencies [d21228a]
- Updated dependencies [4df3509]
- Updated dependencies [428e0a7]
- Updated dependencies [8a28aa1]
- Updated dependencies [a08ab26]
- Updated dependencies [08a87a7]
- Updated dependencies [3861bfd]
- Updated dependencies [9208a20]
- Updated dependencies [53e1e41]
- Updated dependencies [3665a2c]
- Updated dependencies [b7342d2]
- Updated dependencies [e06506b]
- Updated dependencies [f31d22c]
- Updated dependencies [8857666]
- Updated dependencies [1484e3a]
- Updated dependencies [48ad23c]
- Updated dependencies [c491e28]
- Updated dependencies [c667040]
- Updated dependencies [92b28e3]
- Updated dependencies [1d76f93]
- Updated dependencies [29f9e9b]
- Updated dependencies [bc958b9]
- Updated dependencies [8b9b483]
- Updated dependencies [5260e32]
- Updated dependencies [404f117]
- Updated dependencies [ef422d1]
- Updated dependencies [5db9958]
- Updated dependencies [d60eaad]
- Updated dependencies [991ef36]
- Updated dependencies [672e2f8]
- Updated dependencies [89b7b38]
- Updated dependencies [699ae65]
- Updated dependencies [04f41b2]
- Updated dependencies [b9efc3b]
- Updated dependencies [833d912]
- Updated dependencies [892990a]
  - @lumiscia/lumen-bindings@0.5.0
  - @lumiscia/lumen-preview@0.5.0

## 0.4.0

### Minor Changes

- 9494d2c: The preview now has one transport and one clock: a session always renders in a worker, on a canvas it creates inside a host element, so remounts and replaced compositions can't fail on a canvas that was already transferred.

  - `Player` renders a `<div>`, and `className`, `class` and `style` apply to it. The canvas fills it and keeps its aspect ratio.
  - `compositionJson` becomes `composition`, an object or anything with `toJSON()`. `mediaSources` and `audioSources` become one `media` map of alias to source, with kinds inferred from the composition. The new `fonts` map registers fonts by family.
  - `session.attach` takes the host element and never rejects. `mountPreview(host, options)` creates and attaches a session. Failures reach `error`, with a typed `errorCode`: `audio_failed`, `load_failed`, `render_failed`, `webgpu_unavailable` or `worker_failed`.
  - A session needs bindings that name a worker, so `bindings` is now `LumenWorkerBindings`. There is no main-thread driver and no fallback to one. `createLumenPreviewRuntime` stays for code that drives a controller directly.
  - `preview.seek` returns a promise: true once the frame is on screen or a newer seek's is, false if a load or failure got in first. `preview.controller` and `preview.reset()` are gone.
  - A failed load isn't retried until an input changes.
  - A composition too slow for its frame rate drops frames and still answers pause and seek.
  - `dispose` stops a worker still starting and settles `attach` at once, and options changed during startup apply once the worker is ready.
  - A playback holds for audio for about a second. If none has played by then, as on a page with no user gesture or with a mixer that sends nothing or sends late, video runs on elapsed time for the rest of that playback, and audio joins at that position when it arrives, without playing anything older. Once audio has played on time, it leads the clock and holds through an underrun, ramping back up when it returns. A source change more than a second into a playback spends the hold, so that change and any later underrun in the playback are silent gaps instead.
  - The wasm `LumenPreviewController` loses `play`, `pause`, `isPlaying`, `tick`, `targetFrameDurationMs` and `targetFrameForTimeMs`, and the runtime controller loses `tickAsync`. `renderNowAsync` and `setFrame` remain, and `prepareFrame` and `renderPrepared` are new.
  - In Svelte, read state from `preview.state`, a single reactive snapshot. `preview.frame`, `preview.isPlaying` and the other mirrored fields are gone. `PlayerProps` and `MediaRegistration` are exported from the root, and declarations are generated.

- 3b7959a: `LumenCanvas` is now `Player`, and `LumenCanvasProps` is now `PlayerProps`, in `@lumiscia/lumen-react` and `@lumiscia/lumen-svelte`. Its props are unchanged.

### Patch Changes

- Updated dependencies [b4df9b5]
- Updated dependencies [aeca11d]
- Updated dependencies [f5b04fa]
- Updated dependencies [4891f45]
- Updated dependencies [9494d2c]
- Updated dependencies [cb03d75]
- Updated dependencies [37823f6]
  - @lumiscia/lumen-preview@0.4.0

## 0.3.1

### Patch Changes

- Updated dependencies [c44cca1]
- Updated dependencies [b506afc]
  - @lumiscia/lumen-preview@0.3.1

## 0.3.0

### Patch Changes

- Updated dependencies [482ec57]
- Updated dependencies [e030363]
  - @lumiscia/lumen-preview@0.3.0

## 0.2.2

### Patch Changes

- @lumiscia/lumen-preview@0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.

### Patch Changes

- Updated dependencies [072169a]
- Updated dependencies [cb6f4dc]
  - @lumiscia/lumen-preview@0.2.0

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.

### Patch Changes

- Updated dependencies [906481a]
  - @lumiscia/lumen-preview@0.1.0
