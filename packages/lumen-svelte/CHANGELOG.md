# @lumiscia/lumen-svelte

## 0.6.0-beta.0

### Minor Changes

- 6528c7b: The browser preview draws `model_3d`. Pass each model's `.glb` file in `media`, under the name its `source` uses, as you would an image:

  ```ts
  const fox = composition.addNode({
    type: "model_3d",
    params: { source: "fox", animation: "Walk", scale: [0.01, 0.01, 0.01] },
  });
  // ...wire `fox` into a merge_3d or render_3d as usual.

  mountPreview(host, {
    preview,
    composition,
    media: { fox: "/models/fox.glb" },
    wasmUrl,
    workerUrl,
  });
  ```

  A name that a `model_3d` uses is read as a model, and so is a blob of type `model/gltf-binary`; a source that only an expression names needs `{ kind: "model", source }` when neither its name nor its blob's type says. An `asset:<id>` source is a `media` name like any other. A `model_3d` or `media_in` inside a `compositions` template now says what its `media` entry holds just as one at the root does, so an image a template reads no longer needs a `kind` either.

  Models load with the composition, as images do: the preview finishes loading, and draws its first frame, once every model has arrived, and until then keeps showing what it showed before. A model that can't be fetched fails the load with `load_failed`, naming the source. A file that isn't a model Lumen can read fails the load with the `media.*` code a render would give, before anything is drawn. A model left out of `media` is released, along with its GPU copy.

  The preview now parses the composition before it fetches any media, so a composition the engine rejects fails at once instead of after its media has loaded. A preview built without 3D support reports `node.unsupported` for a `model_3d` without fetching its file.

- 6487216: `@lumiscia/lumen-preview` exports only what an application embedding a preview needs. The media bridge, the audio engine and the controller plumbing are no longer public, and the bindings contract moves to a new `@lumiscia/lumen-preview/advanced` subpath.

  The root entry now exports:

  - `createLumenPreview`, `LumenPreviewContext`, `mountPreview`, `LumenPreviewSession`
  - the state and error types: `LumenPreviewState`, `LumenPreviewError`, `LumenPreviewErrorCode`, `LumenPreviewListener`, `LumenPreviewMediaState`, `LumenPreviewPatch`, `LumenPreviewTransport`
  - the option and input types: `LumenPreviewSessionOptions`, `LumenPreviewSessionInputs`, `LumenPreviewComposition`, `LumenPreviewMedia`, `LumenPreviewMediaEntry`, `LumenPreviewMediaKind`, `LumenPreviewFonts`, `LumenPreviewRange`, `LumenLogLevel`, `LumenAudioEngineOptions`
  - the callback types: `LumenPreviewStats`, `LumenPreviewStatsCallback`, `LumenPreviewTimeUpdate`, `LumenPreviewTimeUpdateCallback`
  - `MediaFailure`, the type of `LumenPreviewError.media`, re-exported from `@lumiscia/lumen-types`
  - the source types a `media` or `fonts` entry takes: `MediaSourceInput`, `BlobMediaSourceInput`, `BufferMediaSourceInput`, `UrlMediaSourceInput`

  `@lumiscia/lumen-preview/advanced` exports only types, the contract of a custom host built on `@lumiscia/lumen-preview/worker-host`: the bindings it supplies (`LumenBindings`, `LumenPreviewBindings`, `LumenPreviewControllerBinding`, `LumenMediaTarget`) and the types the worker's messages carry (`VideoSourceRegistration`, `PreviewClock`). The worker's `load` message carries `[source, entry]` pairs of the page's `media`, and the worker works out what to load. `@lumiscia/lumen-bindings` implements the bindings. Every type that appears in a signature of the root, `/advanced`, `./preview` or `./worker-host` can be imported from the root or `/advanced`, apart from types of `@lumiscia/lumen-types` and `mediabunny`.

  `LumenPreviewMediaEntry` now lists its `fps`, `formats` and `track` fields itself instead of extending `VideoSourceRegistration`; the shape is the same.

  Removed from the root entry:

  | Removed                                                                                                                                                                                                                                                                                                                                               | Where it went                      |
  | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
  | `LumenAudioEngine`, `defaultAudioWorkerUrl`, `defaultAudioWorkletUrl`, `AudioSourceRegistration`                                                                                                                                                                                                                                                      | Internal                           |
  | `createLumenPreviewRuntime`, `LumenPreviewRuntime`, `LumenPreviewController`, `resolveLumenPreviewBindings`, `LumenPreviewBindingSource` (and the controller's `registerImageSource`, `registerModelSource`, `registerVideoSource`, `registerVideoSourceBytes`, `loadVideoFrame`, `loadFrameRequirements` and `renderNowAsync`, which nothing called) | Removed                            |
  | `ImageSourceRegistration`, `VideoFrameMetadata`, `VideoFramePayload`, `VideoFramePixels`, `DecodedImageFrame`, `FrameRequirementsPayload`, `FrameRequirementsVideoPayload`, `LumenMediaBridgeOptions`, `NativeVideoFrameTarget`, `MediaRegistration`, `RegisteredImageSource`, `RegisteredModelSource`, `RegisteredVideoSource`                       | Internal                           |
  | `VideoSourceRegistration`, `LumenMediaTarget`                                                                                                                                                                                                                                                                                                         | `@lumiscia/lumen-preview/advanced` |
  | `LumenBindings`, `LumenPreviewBindings`, `LumenPreviewControllerBinding`                                                                                                                                                                                                                                                                              | `@lumiscia/lumen-preview/advanced` |

  The `./audio-engine`, `./media` and `./session` subpaths are removed too, and so is everything they exported (`LumenMediaBridge`, `LumenMediaError`, `toUint8Array`, `createVideoTimeline` and the rest of the media bridge). `./preview` and `./worker-host` stay as they were. `@lumiscia/lumen-react` and `@lumiscia/lumen-svelte` no longer re-export the `MediaRegistration` type.

  Migrating:

  | Before                                                                                 | After                                                                                         |
  | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
  | `import type { LumenBindings } from "@lumiscia/lumen-preview"`                         | `import type { LumenBindings } from "@lumiscia/lumen-preview/advanced"`                       |
  | `import { LumenMediaBridge, … } from "@lumiscia/lumen-preview"` or `…/media`           | No replacement. Pass `media` and `fonts` to `mountPreview` or `Player`.                       |
  | `LumenAudioEngine`, `defaultAudioWorkerUrl()`                                          | No replacement. Pass `audio.workerModuleUrl` to `mountPreview` to serve the worker elsewhere. |
  | `createLumenPreviewRuntime(bindings)` and the controller types                         | No replacement. Mount with `mountPreview`, or host the worker with `…/worker-host`.           |
  | `import … from "@lumiscia/lumen-preview/session"`                                      | `import … from "@lumiscia/lumen-preview"`                                                     |
  | `MediaRegistration` from the root, `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte` | No replacement. Pass the page's `media` entries as they are.                                  |

- d63ba1d: The browser preview loads `.cube` LUTs, and a failed render tells the host the engine's code for it.

  - A `color_grade` with `lut_source` works in the preview. Put the `.cube` file in `media` under the name `lut_source` holds, and it loads like a model or an environment map: `pending`, then `ready` or `failed`, counted in `state.media`, awaited by `whenReady()`, fetched abortably and released when the composition stops naming it. It is read in full when it is registered, so a file that isn't a table the engine can use fails the load with the `media.decode`, `media.unsupported` or `media.too_large` a render would give.
  - `LumenPreviewMediaKind` gains `"lut"`. An entry whose source is a `File` named `*.cube`, or a string, `URL`, `Request` or `{ kind: "url" }` source whose path ends in `.cube`, is read as a LUT without a `kind`.
  - The wasm controller gains `setLut`, `hasLut` and `removeLutSource`, and `LumenMediaTarget` gains them as optional members. `setLut` throws, with `media`, on a file it can't read.
  - **A `render_failed` error now carries the engine's code.** Its `diagnostics` hold one `LumenDiagnostic` with that code (`render.gpu`, `render.limit`, `render.node_evaluation` and the others the engine lists), severity `error`, an empty `path` and the message, so `onError` can tell a composition that needs fixing from a GPU fault. It was always empty for a render failure. A render that failed on a media source keeps `diagnostics` empty and says so in `media`. The `render-failed` worker event gains a required `diagnostics` array.

- 7a87028: The browser preview loads every kind of media one way. What the engine lists the composition reading, joined with the page's `media` and `fonts`, decides what is fetched: images, videos, audio, models, environment maps and fonts each go `pending`, then `ready` or `failed`, and are released when the composition stops naming them.

  - `session.whenReady()` and `preview.whenReady()` resolve once a frame of the current composition is drawn and nothing it needs is pending, with the error that stopped it loading, the `render_failed` error of a first frame that couldn't be drawn, an `audio_failed` error, or `null`. They never reject, so check the value; a composition, `media` or `fonts` change starts them again.
  - A model or environment map is read in full before it counts as ready, so a file that isn't one fails the load with the `media.*` code a render would give. A video is ready once its stream is opened; its frames decode as playback reaches them.
  - `state.media` counts the composition's sources as `{ pending, ready, failed }` (the new `LumenPreviewMediaState` type).
  - While anything is pending the preview holds its last frame, at its size, and playback holds too: the playhead stops and the audio pauses until everything has settled. Playback also waits when the frame it is on has spent 100 ms decoding its video frames, so sound can't run ahead of the picture. A load fails as soon as one source fails, and cancels what was still loading for it.
  - Environment maps (`render_3d.environment_map`, Radiance `.hdr` or OpenEXR `.exr`) load from `media` like models. `LumenPreviewMediaKind` gains `"environment"`.
  - Source names follow `lumen-server`'s rules, checked against a fixture both test suites read. The `@lumiscia/lumen-preview` README has the table.
  - `runtimeUrl` on `mountPreview`, `LumenPreviewSession`, and the React and Svelte `Player` names one directory serving `worker.js`, `lumen_wasm_bg.wasm`, `audio-worker.js` and `audio-worklet.js`, and every runtime URL not passed on its own comes from it, with its query kept on each file. With it, `wasmUrl` and `workerUrl` are optional. Without it nothing changes.
  - The wasm controller gains `setEnvironment`, `hasEnvironment` and `removeEnvironmentSource`, and `LumenMediaTarget` gains them as optional members. `setModel` and `setEnvironment` throw, with `media`, on a file they can't read.

  Breaking changes, each where the old behaviour differed between kinds:

  | Before                                                                                                                                | After                                                                                                                                                                                   | Migrating                                                                    |
  | ------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
  | Every `media` entry was fetched, and one whose kind couldn't be told failed the load with "needs a kind".                             | Only the entries the composition reads load. When an expression computes a source, each entry is also read as the kind it reads, unless the entry's `kind` or blob type says otherwise. | Give entries of other kinds a `kind` when an expression computes a source.   |
  | A `kind` on an entry chose how it was read.                                                                                           | A `kind` the composition doesn't read the source as fails as `media.wrong_kind`.                                                                                                        | Drop the `kind`, or fix it to the kind the node reads.                       |
  | A source written as a URL (`https://…`), with another `scheme://`, empty or with a `..` part loaded if `media` had an entry under it. | It fails as `media.not_found`, as it does on `lumen-server`.                                                                                                                            | Name the source (`intro.mp4`) and pass the URL in `media` under that name.   |
  | Images, videos, models and fonts held the last frame while a composition loaded, but playback ran on and audio loaded on its own.     | Every kind holds the frame, and playback pauses while any source, audio included, is pending.                                                                                           | None needed. Await `whenReady()` before `play()` to start without the pause. |
  | Every `media` and `fonts` entry stayed registered while the page passed it, named by the composition or not.                          | Only what the composition names stays registered; the rest is unregistered and its memory freed. A source the old and new compositions share stays loaded.                              | None needed.                                                                 |
  | `LumenPreviewTransport` had `pause`, `play` and `seek`.                                                                               | It also has `whenReady(): Promise<LumenPreviewError \| null>`.                                                                                                                          | A custom transport passed to `preview.attach()` implements `whenReady`.      |
  | `wasmUrl` and `workerUrl` were required on `LumenPreviewSessionOptions`.                                                              | Both are optional when `runtimeUrl` is set; without either, the session fails to start with `worker_failed`.                                                                            | None needed.                                                                 |

- 1badd98: The preview player can loop, change speed, change volume, play a section and tell you when it ended. `mountPreview`, `session.update()` and the React and Svelte `Player` take these, and apply them live without a remount or a restart:

  - `loop`: plays on from the start of the range at its end, with no gap and no repeated or dropped frame, audio included.
  - `rate`: 0.25 to 4. Audio follows it, with its pitch (it is resampled, not time-stretched). Changing it while playing keeps the current time.
  - `volume` (0 to 1) and `muted`: applied in a gain node over about 50 ms, so they don't click. Un-muting restores the volume.
  - `range: { in, out }`: plays frames `in` to `out - 1` (`out` is exclusive). Seeks land inside it, lookahead decoding stays inside it, and `null` restores the whole timeline. An `out` past the end is cut to it with a console warning.
  - `onEnded`: called once when playback reaches the end of the range without `loop`.
  - `onTimeUpdate({ frame, timeMs })`: called at most once for each frame shown. It costs nothing while unset and doesn't re-render a React or Svelte component.

  A value that can't be used (a `rate` outside 0.25 to 4, a `volume` outside 0 to 1, a malformed `range`, or a non-finite number) is reported through `onError` and the state's `error` with the new code `"invalid_option"`, and the whole update is rejected: the previous inputs and options stay in effect. `session.loop`, `rate`, `volume`, `muted` and `range` read the options in effect.

  ```tsx
  <Player
    preview={preview}
    composition={composition}
    range={{ in: 30, out: 90 }}
    loop
    rate={1.5}
    volume={0.8}
    onEnded={() => setPlaying(false)}
  />
  ```

  **Breaking:** playback no longer loops by default. Before, the preview wrapped round the whole composition forever; now it stops on the last frame and calls `onEnded`, and playing again from there starts over. Pass `loop` to keep the old behaviour. `LumenPreviewErrorCode` gains `"invalid_option"`, so an exhaustive `switch` over it needs a new case.

### Patch Changes

- Updated dependencies [ec34c26]
- Updated dependencies [d09b553]
- Updated dependencies [a146bbd]
- Updated dependencies [4bacce6]
- Updated dependencies [8ef4ee5]
- Updated dependencies [97c7016]
- Updated dependencies [228aa26]
- Updated dependencies [c4d3140]
- Updated dependencies [c657075]
- Updated dependencies [ad144c3]
- Updated dependencies [0b400af]
- Updated dependencies [bba7e08]
- Updated dependencies [40e1f4e]
- Updated dependencies [c4d3f90]
- Updated dependencies [dc06fed]
- Updated dependencies [b577157]
- Updated dependencies [d3e4ce9]
- Updated dependencies [60b7798]
- Updated dependencies [716f9d0]
- Updated dependencies [0aead67]
- Updated dependencies [89568da]
- Updated dependencies [b1f3863]
- Updated dependencies [16aae16]
- Updated dependencies [6528c7b]
- Updated dependencies [b9cc4f1]
- Updated dependencies [a37b057]
- Updated dependencies [6487216]
- Updated dependencies [1879afe]
- Updated dependencies [5ab096d]
- Updated dependencies [d247928]
- Updated dependencies [d63ba1d]
- Updated dependencies [7a87028]
- Updated dependencies [1badd98]
- Updated dependencies [267dca6]
- Updated dependencies [c0f96e6]
- Updated dependencies [5233133]
- Updated dependencies [fe146b5]
- Updated dependencies [79817d9]
- Updated dependencies [bc6b572]
- Updated dependencies [0382574]
- Updated dependencies [f045a94]
- Updated dependencies [ef94bcb]
- Updated dependencies [91a0446]
- Updated dependencies [49b2878]
- Updated dependencies [3ab8aa0]
- Updated dependencies [b9022b7]
  - @lumiscia/lumen-bindings@0.6.0-beta.0
  - @lumiscia/lumen-preview@0.6.0-beta.0

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
