# @lumiscia/lumen-svelte

## 0.5.0

### Patch Changes

- 92b28e3: Every package now has a README, so its npm page describes it.
- Updated dependencies [92b28e3]
- Updated dependencies [bc958b9]
- Updated dependencies [8b9b483]
- Updated dependencies [5260e32]
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
