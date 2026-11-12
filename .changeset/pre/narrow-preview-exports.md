---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

`@lumiscia/lumen-preview` exports only what an application embedding a preview needs. The media bridge, the audio engine and the controller plumbing are no longer public, and the bindings contract moves to a new `@lumiscia/lumen-preview/advanced` subpath.

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
