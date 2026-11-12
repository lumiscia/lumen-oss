# @lumiscia/lumen-preview

## 0.6.0-beta.0

### Minor Changes

- 8ef4ee5: Audio clips fade, volumes animate, and templates carry their own sound.

  - A track's or clip's `volume` takes an `=expression` as well as a number, evaluated in the time of the composition the audio is in: `frame`, `time`, `fps`, `width`, `height` and, inside a template, props. Gains are evaluated every 512 samples and move linearly in between, so they never step. A volume expression that reads a node, a glyph or text, or a prop the template doesn't declare, fails the check with `audio.invalid`; a value below 0 plays as silence.
  - Clips take `fade_in_ms` and `fade_out_ms`, which fade from and to silence at the clip's start and end on an equal-power curve. Fades longer than a short clip multiply where they overlap.
  - A template in `compositions` can have its own `audio`. It plays wherever a `clip` that reaches the output plays the template, at the clip's start, duration, offset, speed and end, as its frames do, through nested clips. Speed resamples, so pitch follows it; past a `hold` window the audio is silent. A `switch` plays only its selected layer's audio, and a `time_remap` plays its source's audio at the remapped time, silent where that time holds or runs backwards.
  - Audio planning is bounded. The root and each template may hold at most 512 audio tracks and 8192 audio clips, and clips that play templates with sound more than 4096 times in all, counting the clips inside the templates they play, fail the check with `audio.invalid`; planning an interval only looks at the clips that overlap it. While rendering or previewing, a 512-sample interval may be inside at most 1024 clips and time remaps (a loop may restart at most 32 times in it) and sound at most 1024 clips, one planning call (the server mixes 30 s at a time) returns at most 500,000 segments, and the preview's plan of the whole timeline covers at most about 3 hours and 200,000 segments; past a limit the render or the preview load fails with the new `audio.limit` code.
  - A prop a clip sets by expression moves a gain smoothly, as the gain's own expression does. Nested audio keeps sounding after a parent loop restarts, including under a `switch` or another loop, and a looping `time_remap` no longer silences the 512 samples around its wrap, whatever its loop bounds.
  - The builder's `addAudio` takes `fadeIn` and `fadeOut` in seconds and `gain` as a number or an `Expr<number>`, `addAudioTrack` takes an `Expr<number>` gain, and `defineComposition` keeps the audio its `build` adds.

  ```json
  {
    "compositions": {
      "whoosh": {
        "timeline": { "fps": 30, "duration_frames": 30 },
        "props": { "level": { "type": "number", "default": 1 } },
        "audio": {
          "tracks": [{ "id": "main" }],
          "clips": [
            {
              "id": "a",
              "source_id": "whoosh.wav",
              "track_id": "main",
              "start_ms": 0,
              "duration_ms": 500,
              "fade_out_ms": 100,
              "volume": "=prop('level')"
            }
          ]
        },
        "nodes": [
          { "id": 1, "type": "background" },
          { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
      }
    },
    "audio": {
      "tracks": [{ "id": "music", "volume": "=if(time >= 2 && time < 6, 0.25, 1)" }],
      "clips": [
        {
          "id": "bed",
          "source_id": "music.mp3",
          "track_id": "music",
          "start_ms": 0,
          "duration_ms": 10000,
          "fade_in_ms": 1000,
          "fade_out_ms": 1000
        }
      ]
    }
  }
  ```

  The browser preview plays the same mix: the engine plans it and the preview's audio worker plays the plan.

  Breaking changes:

  - `audioTimeline()` on the preview's wasm controller (`LumenPreviewControllerBinding` in `@lumiscia/lumen-preview/advanced`) returns the engine's audio plan, `{ voices, segments, durationFrames, fps }`, instead of the parsed tracks and clips. The preview's audio engine is internal and takes the plan itself; only a custom host that reads `audioTimeline()` is affected.
  - The generated `AudioClip` and `AudioTrack` types have `volume: ExpressionValue<number>`, and the root `audio` is the new `AudioTimeline` type.
  - In Rust, `AudioMixer::new` takes the `Composition` instead of its timeline, and `mix_range` takes `&mut self` and fails with a `LumenError`. `AudioTrack::volume` and `AudioClip::volume` are an `AudioGain`, and `CompositionTemplate` has an `audio` field. Use `Composition::has_audio` to ask whether a render has sound.

- 97c7016: **Breaking: `color_grade` has one LUT scale, and its inline LUT is the `lut` param.** `color_grade` also reads 3D `.cube` LUTs from files with the new `lut_source` param.

  - The inline 1D LUT, which was `lut_source`, is now `lut` (`"rgb1d:0,0,0;0.2,0.1,0.3;1,1,1"` or `"identity"`). Rename the param in existing compositions.
  - Its scale is 0 to 1. The engine no longer guesses per value whether a number above 1 was meant as 0–255. A value outside 0..1 or not finite fails the render, naming `lut` and the value. **Migration:** divide a 0–255 LUT by 255 (`rgb1d: 0,0,0; 255,128,0` becomes `rgb1d: 0,0,0; 1,0.5,0`).
  - `lut_source` is now a media source, a path, `media` alias or `asset:<id>`, naming an Adobe/Resolve `.cube` file. It reads `TITLE`, `LUT_3D_SIZE`, `DOMAIN_MIN` and `DOMAIN_MAX`, comments and blank lines, and uploads the table as a 3D texture once per media identity. Lookups are trilinear, with the half-texel offset so the end points land on the first and last entries; `interpolation: "nearest"` takes the nearest entry. `strength` blends the graded colour as it does for `lut`. Table values outside 0..1 are kept.
  - The lookup is in the sRGB-encoded domain on straight colour: the colour is unpremultiplied, encoded, clamped to 0..1, looked up, decoded and premultiplied again, and alpha is untouched.
  - A file is read as untrusted input: `LUT_3D_SIZE` up to 129, files up to 64 MiB, and a missing or wrong entry count, a non-finite or out-of-range number, a missing size line, and any other keyword (`LUT_1D_SIZE`, `LUT_3D_INPUT_RANGE`, ...) are errors that say which. 1D `.cube` tables are not supported.
  - Setting both `lut` and `lut_source` is an error. A literal `lut` that is out of range or malformed, both params set, and an inline LUT written in `lut_source` (as old compositions had it) are `node.invalid_param` diagnostics at the param when the composition is checked; an expression-valued param fails at render with the same message.

  **Rust API:** `AssetKind::Lut`, `Asset::Lut`, `MediaResolver::Lut`, the `LutResolver` trait and `MAX_LUT_BYTES` are new, and `#[media(lut)]` declares a param of that kind. A `MediaStore` that matches on `AssetKind` handles the new variant (a store with no LUTs answers `media.not_found`). `lumen_gpu::LayoutKind::Texture3d` binds a filterable 3D texture.

- 89568da: A node param is declared as a media source in one place, and a composition can list what it needs before it renders.

  A param struct marks a `String` param with `#[media(image, video)]`, `#[media(model)]` or `#[media(environment)]`, and `by = kind` names the enum param that picks among several kinds. `media_in.source`, `model_3d.source`, `render_3d.environment_map` and an audio clip's `source_id` are declared this way. Asset validation, the schema and the browser preview now read the declaration instead of each knowing which nodes take sources.

  `Composition::media_dependencies()` returns the distinct sources a composition's nodes and audio clips name as written, with the kinds each use reads and where it is used (node and param, or audio clip, inside a template too). A source an expression computes is listed as dynamic, and the font families text uses are listed apart from media. The browser bindings expose it as `mediaDependencies(compositionJson)`, typed as `MediaInventory` in `@lumiscia/lumen-types`.

  The generated schema gives each such param `x-lumen-media` (`{ "kinds": ["image", "video"], "selectedBy": "kind" }`), and `@lumiscia/lumen-types` exports the generated `nodeMediaSources` table (node kind → param → kinds) with `MediaSourceParam` and `MediaSourceKind`. The preview reads it to tell what a `media` entry holds.

  **Breaking for Rust:** `PropertyDef` has a `media` field, and the `Node` and `NodeParams` traits have a required `for_each_media_source`; hand-written implementations must add it (the `Node` and `Delegate` derives do). `lumen_engine::media::resolve_asset_of` accepts any of several kinds. `LumenPreviewBindings` has a required `mediaDependencies`, so a bindings object built by hand must add it. Compositions change in one way only: a name in `compositions` may be at most 256 bytes long; a longer one fails with `document.invalid`.

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

- a37b057: A model's animation clips and materials can be listed, so `animation` and `texture_material` no longer have to be guessed from an error.

  An uploaded model's media object has `kind: "model"` and a `metadata` listing them, read once when the upload completes and returned by `GET /media/:id`:

  ```json
  {
    "kind": "model",
    "metadata": {
      "animations": [
        { "index": 0, "name": "Walk", "duration": 0.7 },
        { "index": 1, "duration": 2 }
      ],
      "materials": [{ "index": 0, "name": "screen", "hasBaseColorTexture": false }],
      "cameras": []
    }
  }
  ```

  In the browser, `modelInfo` on the bindings reads a `.glb` and returns the same lists, throwing on a file a render could not read:

  ```ts
  const { modelInfo } = await createLumenBindings().preview();
  const { animations, materials } = modelInfo(new Uint8Array(await file.arrayBuffer()));
  ```

  `TemporaryMedia` has `kind` and `metadata` (`MediaKind`, `MediaMetadata`), `@lumiscia/lumen-types` has `LumenModelInfo`, and `uploadMedia` knows `.glb` file names. The error for an `animation` or `texture_material` the model lacks now lists at most 20 names, then how many more there are.

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

- 1879afe: Every media source that can't be used fails the same way, for images, videos, audio, fonts, models and environment maps, in the engine, `check`, `lumen-server`, `lumen` and the preview.

  A failure names the source as the composition wrote it, the kinds the place using it reads, the kind it is when that is established (a container such as MP4 or WebM can't be told as video or audio from its header, so it has no `found` until it is opened), where it is used (node and param, audio clip, template), and why, with one of five public codes:

  | Code                | Means                                                                                                                                       |
  | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
  | `media.not_found`   | Nothing is at the source: no file, no URL answer, no asset with that id                                                                     |
  | `media.wrong_kind`  | The source is another kind of media than the use reads, such as a video in an `image` node or a font that is a PNG                          |
  | `media.unsupported` | The engine identifies the data (a container, codec, glTF extension, `.gltf` JSON, WOFF) and doesn't read it                                 |
  | `media.too_large`   | Over a limit (file bytes, pixels, the device's texture size, a model's counts), with the `limit` and the `actual` value when they are known |
  | `media.decode`      | The data is corrupt, truncated, contradicts itself, or is in no format the engine identifies                                                |

  `media.decode` and `media.frame_out_of_range` keep their codes. `check` reports missing and wrong-kind `asset:` references with these codes and the JSON pointer of the param or audio clip, also inside templates. `lumen-server` carries the engine's code through unchanged, and marks a render retryable only for a shortage that can pass (out of memory or file descriptors, `EAGAIN`, a busy device), in the engine's reads, in FFmpeg's open and decode, and while resolving a path. Failures after a source opened name it as the composition wrote it, and the HTTP error message never shows a path on the server. A `.gltf` JSON file is `media.unsupported` in renders and in the upload probe; other text that starts with `{` is `media.decode`. The upload probe's rejection reasons end with the code, such as `(media.too_large)`. `lumen` prints `lumen failed (media.not_found): ...`.

  The thrown error of the WASM bindings carries `code` (the dotted code for a media failure) and `media` (the structured fields). `LumenPreviewError` has a new `media` field with them, typed as `MediaFailure` in `@lumiscia/lumen-types`, and it is `null` for other failures. The preview's own reasons agree with the engine's: an audio-only file registered as video is `media.wrong_kind` (found `audio`), a video the browser can't decode is `media.unsupported`, and a file it can't open or identify is `media.decode`.

  Migrating:

  | Before                                                                         | After                                                                                           |
  | ------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
  | `asset.not_found` (check, render)                                              | `media.not_found`, with the source `asset:<id>`                                                 |
  | `asset.wrong_kind` (check, render)                                             | `media.wrong_kind`                                                                              |
  | `media.not_found` for a missing file, URL, font or model                       | `media.not_found` (unchanged), now with the kinds wanted and where it is used                   |
  | `media.decode` for a file of another kind, an unsupported codec or a size cap  | `media.wrong_kind`, `media.unsupported` or `media.too_large`; corrupt data stays `media.decode` |
  | Rust `MediaError::{SourceNotFound, Decode, AssetNotFound, AssetWrongKind}`     | `MediaError::Failed(MediaFailure)`; build one with `MediaError::source_not_found` or `decode`   |
  | The 3D crate's `ModelError::TooLarge(String)` and `EnvironmentError::TooLarge` | `TooLarge { details, limit, actual }`                                                           |
  | Messages such as ``failed to decode image `a.png`: ...``                       | ``image `a.png` (node 3 `source`) failed to decode: ...``; match the `code`, not the message    |
  | An upload probe's rejection reason                                             | The same reason followed by its code, such as `... (media.decode)`                              |

  ```json
  {
    "severity": "error",
    "code": "media.wrong_kind",
    "path": "/nodes/4/params/source",
    "node": 5,
    "message": "`asset:clip` (node 5 `source`) is a video, not an image"
  }
  ```

- d247928: The preview converts colour as a render does. Images are decoded with their embedded colour profile honoured and converted to sRGB explicitly (`createImageBitmap` with `colorSpaceConversion: "default"`, read back through an sRGB 2D canvas), whatever the display, so a Display P3 photo looks as it does in a render within 8-bit rounding. Video frames handed to the engine as YUV planes now carry the source's tagged primaries and transfer, so BT.2020 and Display P3 video is converted to BT.709 and PQ and HLG video is tone-mapped to SDR as a render does, instead of being read as SDR BT.709. A YUV tag the engine doesn't convert, such as a `linear` transfer or unknown primaries, warns once per source with `media.color_unsupported`, as a render does. Video frames the browser hands over as RGB in other primaries or another transfer are drawn on an sRGB canvas, which converts them; sRGB, BT.709 and BT.601 ones are copied as they are.

  The preview is still not pixel-identical to a render. 16-bit images arrive in the browser at 8 bits, and colour outside sRGB is clipped there. Images pass through an 8-bit canvas that stores colour premultiplied, so straight colour at very low alpha is coarsely rounded, as before. Browsers' decoders and colour management differ, by a code or so.

  `setVideoFrameYuv` on the preview's wasm controller takes two more optional arguments at the end, `primaries` and `transfer`, the source's WebCodecs tags; a host calling it directly that leaves them out gets BT.709 SDR as before.

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

- d09b553: Documentation for 3D. The `lumen-sdk`, `lumen-shared` and `lumen-preview` READMEs link to the new 3D guide, and `lumen-shared`'s shows a scene. The descriptions of a 3D material's `color` and `shadow_catcher` params, which `lumen-types` carries as doc comments and `lumen-bindings` as node metadata, now say that only an image plane has a raster to multiply by `color` or to count in `shadow_catcher`. Nothing about how a composition renders changes.
- Updated dependencies [ec34c26]
- Updated dependencies [d09b553]
- Updated dependencies [4bacce6]
- Updated dependencies [8ef4ee5]
- Updated dependencies [97c7016]
- Updated dependencies [c4d3140]
- Updated dependencies [c657075]
- Updated dependencies [c4d3f90]
- Updated dependencies [dc06fed]
- Updated dependencies [d3e4ce9]
- Updated dependencies [89568da]
- Updated dependencies [16aae16]
- Updated dependencies [b9cc4f1]
- Updated dependencies [a37b057]
- Updated dependencies [1879afe]
- Updated dependencies [c0f96e6]
- Updated dependencies [5233133]
- Updated dependencies [fe146b5]
- Updated dependencies [ac4bc51]
- Updated dependencies [0382574]
- Updated dependencies [f045a94]
- Updated dependencies [ef94bcb]
- Updated dependencies [49b2878]
- Updated dependencies [3ab8aa0]
  - @lumiscia/lumen-types@0.6.0-beta.0

## 0.5.1

### Patch Changes

- Updated dependencies [6285ab0]
- Updated dependencies [87164d8]
  - @lumiscia/lumen-types@0.5.1

## 0.5.0

### Minor Changes

- 4df3509: Diagnostics reach the TypeScript packages. The wasm bindings add `check(json, fonts?)`, which returns what is wrong with a composition without loading it, and a composition `loadComposition` rejects throws with the same `diagnostics`. `LumenError` in the SDK has `diagnostics`, filled when a render is rejected for an invalid composition. The preview and both `Player` components take `onError`, which receives each failure with its diagnostics. `LumenDiagnostic` is exported from `@lumiscia/lumen-types`.
- 29f9e9b: `Player` no longer takes `bindings`: `<Player preview={preview} composition={composition} />` is enough. React and Svelte now depend on `@lumiscia/lumen-bindings` and start its preview worker and wasm from where they're installed, which Vite and plain ESM both resolve. A page that serves them elsewhere passes `workerUrl` and `wasmUrl` to `Player`.

  This is breaking. `mountPreview` and `LumenPreviewSession` take `wasmUrl` and `workerUrl` in place of `bindings`, and `@lumiscia/lumen-bindings/urls` exports the defaults without loading the wasm. `LumenWorkerBindings` is gone, and `createLumenBindings` from `@lumiscia/lumen-bindings` no longer takes `previewWorkerUrl`; it only loads the wasm on the calling thread.

- 8b9b483: Updating a preview's composition now keeps the playback position instead of rewinding to frame 0, and a preview that was playing keeps playing. The worker starts the new composition on the clock it was already following, so frequent updates, such as from a dragged slider, neither hold playback back nor step the frame backwards. A paused position past the new end parks on the last frame, and playback carries on round a shorter loop. Audio with an unchanged timeline plays on undisturbed. A changed audio timeline restarts the mix at the current position. A seek made during an update applies once it loads, clamped to the new length.

### Patch Changes

- 92b28e3: Every package now has a README, so its npm page describes it.
- bc958b9: When audio starts and then stops arriving for good, the preview no longer freezes silently. After 10 seconds of continuous underrun during playback it reports `audio_failed` once and keeps the video playing without audio until the next play, seek or composition load.
- 5260e32: The preview hands video to the engine as the browser's native YUV planes, and the engine converts them with the same matrix as a server render, instead of leaving the conversion to the browser. Untagged video now looks the same in the preview and in a render: below 720 rows it's BT.601, and from 720 rows it's BT.709. Chrome used to pick BT.709 for sizes like 960×540 and 720×576, and Safari for everything, including video tagged BT.601. Video also shows in Safari again, which ignored the RGBA conversion and left frames black. Rotated video is rotated by the engine.
- Updated dependencies [6cddffa]
- Updated dependencies [db420ff]
- Updated dependencies [d21228a]
- Updated dependencies [4df3509]
- Updated dependencies [428e0a7]
- Updated dependencies [8a28aa1]
- Updated dependencies [08a87a7]
- Updated dependencies [3861bfd]
- Updated dependencies [53e1e41]
- Updated dependencies [3665a2c]
- Updated dependencies [b7342d2]
- Updated dependencies [f31d22c]
- Updated dependencies [8857666]
- Updated dependencies [48ad23c]
- Updated dependencies [c491e28]
- Updated dependencies [92b28e3]
- Updated dependencies [1d76f93]
- Updated dependencies [404f117]
- Updated dependencies [ef422d1]
- Updated dependencies [5db9958]
  - @lumiscia/lumen-types@0.5.0

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
