# @lumiscia/lumen-preview

## 0.4.0

### Minor Changes

- b4df9b5: The frame graph is now the only renderer. The compiled plan path and the `LUMEN_FRAME_GRAPH` flag are gone, so every frame is recorded with pooled textures and cached programs.

  Behaviour changes:

  - A shader that writes fewer pixels than it did on the previous frame no longer leaves the old pixels behind. What it doesn't write is transparent.
  - If a source's first output frame shows it twice and only the second import fails, the render fails with the "would change" error instead of moving both uses to CPU decoding.
  - Video jobs can hold more VRAM.
  - Shaders chosen by an expression build at first use. A bad literal shader still fails at compile time when the output can reach it.
  - `compile()` now checks every branch connected to the output, not only what frame 0 shows, even if no frame ever reaches it. A bad literal shader or uniform under a clip that hasn't started, a zero-duration clip, or any layer of an expression switch, including one like `selected_layer: "=0"`, fails at compile. Disconnected nodes, unplayed templates and the unselected layers of a literal switch still aren't checked.
  - `compile()` no longer evaluates uniform expressions, so one giving the wrong component count fails at the frame that uses it. Literal uniforms are still checked at compile, including the length of a vector with expression components.
  - `render_frame_into_external` accepts more targets: a `COPY_DST` texture of the right size and format no longer needs the other usages. A wrong size or format is still refused, with a reworded error.

  For the preview: the WASM `LumenPreviewController` no longer has `setLookaheadCount`, because there is nothing to build ahead. `setLookaheadCount` on the `@lumiscia/lumen-preview` controller and the `lookaheadCount` prop still set how far ahead media loads.

  For Rust: `lumen-engine-gpu` loses `RenderPlan`, `RenderPlanBuilder` and the plan resources, keeping `Recorder`. `GpuCompositionRenderer` loses `compile_with_media`, `precompile_frame*`, `set_plan_cache_budget`, `compiled` and the plan counters. `plan_cache_bytes` is now `held_bytes` and `cached_pass_count` is now `skipped_pass_count`. `RasterHandle` holds the texture itself, and `ExternalTexture` constructors no longer take a domain. `lumen-local`'s summary prints `held_bytes`, `skipped_passes` and `submitted_passes`.

- f5b04fa: Preview errors are readable and precise. `preview.error` is now the message rather than a stack trace, with the stack still logged to the console. Media failures always name their source, through the message and the new `LumenMediaError.sourceId`. Render failures from the WASM bindings are now `Error` objects rather than strings, and a video frame that isn't loaded yet has `code: "media_frame_out_of_range"`. Only that code makes the preview load the frame and render again, so an unrelated error that happens to contain the words "media frame" and "out of range" is no longer retried.
- 4891f45: Preview audio now mixes like export and reads the timeline the engine parsed, stops as soon as a composition or media reload begins, and reports sources that fail to load instead of failing silently. Replaced or removed media is cancelled and can no longer overwrite newer sources. The package no longer ships its tests. Decoded frames and inputs no longer leak, and failed media loads can be retried. Breaking: `timelineMode`, `native-samples`, the `audioTimeline` session option, Mediabunny `Source` inputs and the audio schedule and parsing helpers are gone, and a video with no `fps` and no readable frame rate is now an error.
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

- cb03d75: Removes preview exports nothing used. `createLumenPreviewRuntime` now returns only `LumenPreviewController`, and `LumenMediaStore`, `LumenRenderer` and `registerFontFamilyBytes` (use `registerFontFamily`) are gone from `@lumiscia/lumen-preview` and `@lumiscia/lumen-bindings`. The controller drops `togglePlay` (use `play` and `pause`), `frameRequirementsWindow`, `snapshot`, `setCompositionSyncMs` and `recordRenderTiming`. The media helpers, such as `LumenMediaBridge`, `fetchBlob` and `normalizeFps`, are imported from `@lumiscia/lumen-preview/media` instead of the package root.
- 37823f6: `@lumiscia/lumen-bindings` ships one wasm build instead of four.

  - `.` is now `/browser`, which works without a bundler and takes optional `previewWasmUrl` and `previewWorkerUrl` overrides. A pre-bundled preview worker ships at `/browser/worker.js`.
  - `/bundler` stays as the Vite entry and needs no configuration. Remove `vite-plugin-wasm`, `vite-plugin-top-level-await` and any `optimizeDeps.exclude` for these packages.
  - `/node` loads the same wasm with `initSync`. It parses compositions; rendering still needs a browser.
  - `/web`, `/nodejs` and `/no-modules` are gone. Use `/browser`, `/node`, or `initSync` from `/browser`.
  - `createLumenBindings` returns a plain object, and the `LumenBindings` class, its `target` field, `LumenBindingsLike` and `LumenPreviewBindingsModule` are gone. Non-URL wasm inputs go through `initSync`.
  - `createLumenPreviewWorkerHost` no longer takes `previewWasmUrl`, because the session sends the worker the URL from its bindings.
  - `preview()` shares one compile across overlapping calls, and an instance already installed stays, including one from `initSync`, so a late init can no longer invalidate live controllers. The one exception: on Chromium's main thread, a wasm over 8 MB installs asynchronously, and it replaces an instance that `initSync` installed from a different, smaller wasm in the meantime.

### Patch Changes

- aeca11d: Rendered output for video inputs changes intentionally: `lumen-local` and `lumen-server` now convert video with its tagged colour matrix and range, where they used to read everything as limited-range BT.601. Untagged video reads as BT.709 from 720 lines up and BT.601 below. Phone video with a rotation flag now renders upright at its display size, native and in the browser preview. Files whose first frame has a non-zero timestamp now show the right frames, and files with several video tracks use the one FFmpeg ranks best. `lumen-local` no longer runs out of memory on long clips, because each video source now caches at most 64 frames or 256 MiB.

  Breaking: a video with no usable frame rate or duration now fails to open with `unable to determine stream frame rate` or `unable to determine stream frame count`, where it used to get an invented frame rate or a one-frame length.

  For Rust: `VideoStreamInfo` gains `rotation`, `frame_rate` and `start_time`, its `width` and `height` are now display dimensions, and `InputContext::duration_us` is new.

- Updated dependencies [c1c57b2]
- Updated dependencies [b6ece6b]
- Updated dependencies [6b3a821]
- Updated dependencies [0003560]
- Updated dependencies [75572c3]
- Updated dependencies [eb354e6]
- Updated dependencies [4f3894f]
- Updated dependencies [93fb454]
- Updated dependencies [5a0939c]
- Updated dependencies [37500c3]
- Updated dependencies [f35c132]
- Updated dependencies [0d47151]
- Updated dependencies [e3d02e6]
- Updated dependencies [ca87eaa]
  - @lumiscia/lumen-types@0.4.0

## 0.3.1

### Patch Changes

- c44cca1: Preview audio now loops with the video. When playback passes the end of the timeline, the audio engine wraps back to 0 at `duration_frames / fps` instead of playing on past the end. Nothing on the audio timeline at or past the end is mixed, and `currentTimeMs()` returns the position within the loop instead of clamping to the duration. With the worker driver, audio and video now jump back to 0 together. With the main-thread driver, video no longer freezes on the last frame while audio carries on. Loop lengths that aren't a whole number of samples, such as at 29.97 fps, stay in phase over repeated loops. A timeline with a zero or invalid duration doesn't wrap.
- b506afc: Play audio clips whose source has no decodable audio, such as a video without an audio track, as silence instead of failing the preview.

## 0.3.0

### Patch Changes

- 482ec57: Update runtime dependencies to their latest releases.
- e030363: Keep preview commands applied while a render is in flight, reuse the GPU device across composition and media changes, and bound the wasm video frame cache by bytes through `frameCacheCapacity`.

## 0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.
