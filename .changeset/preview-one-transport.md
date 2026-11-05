---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

The preview now has one transport and one clock: a session always renders in a worker, on a canvas it creates inside a host element, so remounts and replaced compositions can't fail on a canvas that was already transferred.

- `LumenCanvas` renders a `<div>`, and `className`, `class` and `style` apply to it. The canvas fills it and keeps its aspect ratio.
- `compositionJson` becomes `composition`, an object or anything with `toJSON()`. `mediaSources` and `audioSources` become one `media` map of alias to source, with kinds inferred from the composition. The new `fonts` map registers fonts by family.
- `session.attach` takes the host element and never rejects. `mountPreview(host, options)` creates and attaches a session. Failures reach `error`, with a typed `errorCode`: `audio_failed`, `load_failed`, `render_failed`, `webgpu_unavailable` or `worker_failed`.
- A session needs bindings that name a worker, so `bindings` is now `LumenWorkerBindings`. There is no main-thread driver and no fallback to one. `createLumenPreviewRuntime` stays for code that drives a controller directly.
- `preview.seek` returns a promise: true once the frame is on screen or a newer seek's is, false if a load or failure got in first. `preview.controller` and `preview.reset()` are gone.
- A failed load isn't retried until an input changes.
- A composition too slow for its frame rate drops frames and still answers pause and seek.
- `dispose` stops a worker still starting and settles `attach` at once, and options changed during startup apply once the worker is ready.
- A playback holds for audio for about a second. If none has played by then, as on a page with no user gesture or with a mixer that sends nothing or sends late, video runs on elapsed time for the rest of that playback, and audio joins at that position when it arrives, without playing anything older. Once audio has played on time, it leads the clock and holds through an underrun, ramping back up when it returns.
- The wasm `LumenPreviewController` loses `play`, `pause`, `isPlaying`, `tick`, `targetFrameDurationMs` and `targetFrameForTimeMs`, and the runtime controller loses `tickAsync`. `renderNowAsync` and `setFrame` remain, and `prepareFrame` and `renderPrepared` are new.
- In Svelte, read state from `preview.state`, a single reactive snapshot. `preview.frame`, `preview.isPlaying` and the other mirrored fields are gone. `LumenCanvasProps` and `MediaRegistration` are exported from the root, and declarations are generated.
