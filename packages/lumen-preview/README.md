# @lumiscia/lumen-preview

The framework-independent browser preview for Lumen: playback state, a transport, media decoding and audio. Use it to preview in plain JavaScript, or through `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte`. It needs a browser with WebGPU and `@lumiscia/lumen-bindings`.

```bash
pnpm add @lumiscia/lumen-preview @lumiscia/lumen-bindings @lumiscia/lumen-shared
```

```ts
import { createLumenPreview, mountPreview } from "@lumiscia/lumen-preview";
import { wasmUrl, workerUrl } from "@lumiscia/lumen-bindings/urls";
import { Composition } from "@lumiscia/lumen-shared";

const composition = new Composition({ timeline: { durationSeconds: 3 } });
const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
const title = composition.addNode({
  type: "text",
  params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
});
const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
composition.addNode({ type: "media_output", inputs: { source: merged } });

const preview = createLumenPreview();
mountPreview(document.body, { preview, composition, wasmUrl, workerUrl });

// Loading is asynchronous, and play() before it finishes is ignored.
document.querySelector("button")?.addEventListener("click", () => {
  if (preview.getSnapshot().isLoaded) preview.play();
});
```

## Media

Pass `media` and `fonts` when the composition uses them. `media` maps each source the composition writes (in `media_in.source`, `model_3d.source`, `render_3d.environment_map`, `color_grade.lut_source` or an audio clip's `source_id`) to a URL, a `Blob` or bytes: images, videos, audio, binary glTF (`.glb`) models, Radiance `.hdr` or OpenEXR `.exr` environment maps and `.cube` 3D LUTs. `fonts` maps each font family to its file.

```ts
mountPreview(document.body, {
  preview,
  composition,
  runtimeUrl: "/lumen-runtime/",
  media: {
    "clips/intro.mp4": "/media/intro.mp4",
    logo: { source: logoBlob, kind: "image" },
    fox: "/models/Fox.glb",
    "studio.hdr": "/env/studio.hdr",
    "looks/warm.cube": "/luts/warm.cube",
  },
  fonts: { Inter: "/fonts/Inter.ttf" },
});
```

Every kind, fonts included, loads the same way. The engine lists what the composition reads, and only those entries are fetched; the rest of `media` is left alone. When an expression computes a source, any entry could be read, so every entry is also read as the kind that expression reads, unless its `kind` or blob type says it holds another, or a node already reads it as one of them. An entry that could be either of several kinds says which with `kind` or its blob's type; a `File` named `*.cube`, or a URL or path ending in `.cube`, is a LUT without one.

Each source is `pending` until it is fetched, read and handed to the renderer, then `ready` or `failed`. An image is decoded and a model, environment map or LUT read in full first, so a file that isn't one fails here, with the code a render would give. A video is ready once its stream is opened and its length and size read; its frames decode as playback reaches them, so a corrupt frame later in a video fails the render that needs it. The state's `media` counts the sources, and `whenReady()` resolves once a frame of the current composition is drawn and nothing it needs is pending:

```ts
const error = await preview.whenReady(); // or session.whenReady()
if (error) console.warn(error.code, error.media?.source, error.message);
const { pending, ready, failed } = preview.getSnapshot().media;
```

`whenReady()` never rejects, so check what it resolves with: the error that stopped the composition loading, the `render_failed` error of a first frame that couldn't be drawn, the `audio_failed` error of an audio source that failed while the picture plays on, or `null`. Changing the composition, `media` or `fonts` starts again, and a call made before then resolves for the newest composition.

While anything the composition needs is pending, the preview holds the last frame it showed, at its size. A new composition appears once its images, videos, models, environment maps, LUTs and fonts are ready and its first frame is drawn. It fails to load with the media error of the first that failed, as soon as one does, and what was still loading for it is cancelled. Playback holds too: while any source, audio included, is pending, or the frame playback is on has spent 100 ms decoding its video frames, the playhead stops where it is and the audio pauses. Both start again together once nothing is pending and that frame is drawn, so the sound can't run ahead of the picture.

A source the composition stops naming is released: a read still in flight is cancelled, the renderer unregisters it, and its decoded frames, bitmaps and audio are freed. A source the old and new compositions share stays loaded, and a source whose entry changes loads again.

### Source names

A source is found by its exact name in `media`. These rules match `lumen-server`, which reads sources as paths under its media root, so a composition that loads in one fails the same way in the other:

| Source                                     | Preview                                    | `lumen-server`                     |
| ------------------------------------------ | ------------------------------------------ | ---------------------------------- |
| A name or path, such as `clips/a.mp4`      | the entry under that name                  | that path under the media root     |
| `file://clips/a.mp4`                       | the entry under `file://clips/a.mp4`       | `clips/a.mp4` under the media root |
| `asset:<id>`                               | the entry under `asset:<id>`               | the uploaded asset with that id    |
| `https://…`, `http://…`, other `scheme://` | `media.not_found`, even with such an entry | `media.not_found`                  |
| an empty path, or one with a `..` part     | `media.not_found`, even with such an entry | `media.not_found`                  |
| a name with no entry, or no asset          | `media.not_found`                          | `media.not_found`                  |

The one difference is that the preview has no asset store: an `asset:<id>` resolves only when the page passes that asset's URL in `media` under the same `asset:<id>` name. An entry whose `kind` the composition doesn't read the source as fails as `media.wrong_kind`.

## Runtime files

The preview renders in a worker that loads a wasm module, and plays audio through its own worker and an audio worklet. Pass `runtimeUrl`, a directory that serves all four files under their own names, as the reliable way to host them with any bundler or CDN, with no dev server setting:

| File                 | Copy of                                            |
| -------------------- | -------------------------------------------------- |
| `worker.js`          | `@lumiscia/lumen-bindings/dist/browser/worker.js`  |
| `lumen_wasm_bg.wasm` | `@lumiscia/lumen-bindings/wasm/lumen_wasm_bg.wasm` |
| `audio-worker.js`    | `@lumiscia/lumen-preview/dist/audio-worker.js`     |
| `audio-worklet.js`   | `@lumiscia/lumen-preview/dist/audio-worklet.js`    |

`runtimeUrl` names a directory whether or not it ends with `/`: `https://cdn.example/lumen?v=3` serves `https://cdn.example/lumen/worker.js?v=3`, with its query kept on each file and any fragment dropped. `workerUrl`, `wasmUrl` and `audio.workerModuleUrl` / `audio.workletModuleUrl` still name one file each and win over `runtimeUrl`. To run the smaller 2D-only build, which has no 3D, serve `worker-2d.js` and `lumen_wasm_2d_bg.wasm` from `@lumiscia/lumen-bindings` as well and pass their URLs as `workerUrl` and `wasmUrl`. `runtimeFileUrl(runtimeUrl, name)` gives a file's URL in `runtimeUrl`, and the `/2d` `Player`s of `@lumiscia/lumen-react` and `@lumiscia/lumen-svelte` use it to do this themselves. Without `runtimeUrl`, pass `workerUrl` and `wasmUrl` from `@lumiscia/lumen-bindings/urls`, and the audio files are found next to this package, which works where the bundler emits them as assets; with Vite 6 or 7, the `@lumiscia/lumen-bindings` README has the dev server setting this needs. The files can be on another origin that sends CORS headers. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`.

## Errors

`onError` is called with each failure the state's `error` shows. When the engine rejects the composition, its `diagnostics` say what is wrong and where:

```ts
mountPreview(document.body, {
  preview,
  composition,
  wasmUrl,
  workerUrl,
  onError: ({ code, message, diagnostics }) => {
    for (const { severity, code, path, message } of diagnostics) {
      console.warn(`${severity} ${code} at ${path || "/"}: ${message}`);
    }
  },
});
```

A media source that can't be used (an image, video, audio, font, model, environment map or LUT that is missing, of another kind, unsupported, too large or corrupt) is reported the same way for every kind. `media` on the error says which source, what was wanted, why and where the composition uses it, and its `code` is one of `media.not_found`, `media.wrong_kind`, `media.unsupported`, `media.too_large` or `media.decode`. It is `null` for any other failure:

```ts
onError: ({ media }) => {
  if (!media) return;
  const where = media.at?.type === "node" ? `node ${media.at.node} \`${media.at.param}\`` : "the composition";
  switch (media.code) {
    case "media.not_found":
      console.warn(`${media.source} is missing from ${where}`);
      break;
    case "media.too_large":
      console.warn(`${media.source} is ${media.actual ?? "over"} (limit ${media.limit}): ${media.details}`);
      break;
    default:
      console.warn(`${media.code} ${media.source}: ${media.details}`);
  }
},
```

A frame that fails to render is a `render_failed` error whose `diagnostics` hold one entry with the engine's code for it, such as `render.gpu` for a GPU fault, `render.limit` for a limit the composition exceeds or `render.node_evaluation` for a node that failed, with an empty `path`. A render that failed on a media source has no diagnostic; `media` says which:

```ts
onError: ({ code, diagnostics }) => {
  if (code === "render_failed" && diagnostics[0]?.code === "render.gpu") {
    console.warn("the GPU failed; trying again may work");
  }
},
```

Colour in the preview is converted as a render converts it, but it is not pixel-identical:

- Images are decoded by the browser with their embedded colour profile honoured and converted to sRGB, whatever the display, so a Display P3 or Adobe RGB photo looks as it does in a render. The browser hands them over at 8 bits: a 16-bit PNG or TIFF loses the depth a render keeps, and colour outside sRGB is clipped where a render keeps it for effects such as `exposure` to bring back. Each browser's decoder and colour management round differently, so an image can differ from a render, and between browsers, by a code or so.
- Images, and video frames the browser converts, pass through an 8-bit 2D canvas, which stores colour premultiplied: where alpha is low, straight colour comes back coarsely rounded, as it always has.
- Video frames in a YUV layout the engine converts carry the source's tagged primaries and transfer, so BT.2020 and Display P3 video converts to BT.709 and PQ and HLG video is tone-mapped to SDR as in a render, and a tag the engine doesn't convert, such as a `linear` transfer, gets the same `media.color_unsupported` warning. Frames the browser hands over as RGB in other primaries or another transfer are converted to sRGB by the browser, which decides how to show HDR.

Diagnostic codes are public API, and new ones can appear in a patch release, so handle codes you don't know by their `severity`. The [engine README](https://github.com/lumiscia/lumen-oss/tree/main/crates/engine#diagnostics) lists them.

## Playback controls

`mountPreview` takes these options, and `session.setInputs()` changes them while it plays, without loading or restarting anything. A frame is a frame of the composition's timeline, as in `preview.seek(frame)`.

| Option         | Default | Effect                                                                                                                                                |
| -------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `loop`         | `false` | Plays on from the start of the range at its end, with no gap and no repeated or dropped frame, audio included. Without it playback stops at the end.  |
| `rate`         | `1`     | Playback speed from `0.25` to `4`. Audio follows it, with its pitch: it is resampled, not time-stretched.                                             |
| `volume`       | `1`     | Audio level from `0` to `1`.                                                                                                                          |
| `muted`        | `false` | Silences the audio. `volume` stays, so un-muting restores it. Both change over about 50 ms, without clicks.                                           |
| `range`        | `null`  | `{ in, out }` plays only frames `in` to `out - 1`; `out` is exclusive. `null` plays the whole composition.                                            |
| `onEnded`      | none    | Called once when playback reaches the end of the range without `loop`.                                                                                |
| `onTimeUpdate` | none    | Called with `{ frame, timeMs }` for each frame shown, at most once per frame. It costs nothing while unset, and doesn't re-render anything by itself. |

```ts
const session = mountPreview(document.body, {
  preview,
  composition,
  wasmUrl,
  workerUrl,
  range: { in: 30, out: 90 }, // frames 30 to 89
  loop: true,
  rate: 1.5,
  volume: 0.8,
  onTimeUpdate: ({ frame, timeMs }) => scrubber.set(frame, timeMs),
  onEnded: () => console.log("done"),
});

session.setInputs({ composition, range: { in: 30, out: 90 }, loop: true, rate: 1.5, muted: true });
session.rate; // 1.5: the options in effect are readable as loop, rate, volume, muted and range
```

`setInputs()` replaces every input, so pass the ones that should stay, and it does nothing for inputs that are unchanged, so it is safe to call on every render. A composition is read once per object: pass a new object to change it, not the same one edited in place. Some details:

- A value that can't be used (a `rate` outside `0.25` to `4`, a `volume` outside `0` to `1`, a `range` that isn't whole frames with `0 <= in < out`, or any of them not finite) is reported through `onError` with the code `"invalid_option"` and the state's `error`. The whole call is rejected: none of its inputs (composition, media, fonts, callbacks or other options) are applied, and the earlier ones stay in effect, also while the worker is still starting. Options that are invalid when the session is created are reported the same way and start from their defaults, with the other inputs used as given.
- A `range` with `out` past the end of the composition is cut to the end, with a console warning. An `in` past the end is ignored with a warning, which plays the whole composition. The range is checked again when a different composition loads.
- Seeks land inside the range, whether or not it loops, and a playhead outside a new range moves to its `in`. Clearing the range restores the whole timeline without moving the playhead. Changing `loop` or `range` keeps the position playback had reached, also when a looping playback has already wrapped round and when the audio has stopped arriving.
- Playback that reached the end of the range stops on its last frame and calls `onEnded` once. A pause, a seek or a change of `rate`, `loop` or `range` just before the end doesn't. Playing from the last frame of a range that doesn't loop starts it from `in` again.
- Changing `loop` or `range` while audio plays restarts the audio mix at the current position, which can leave a brief gap in the sound. Changing `rate`, `volume` and `muted` doesn't.
- Lookahead decoding stays inside the range, and wraps round it only when it loops.

## Entry points

`@lumiscia/lumen-preview` exports `createLumenPreview`, `mountPreview`, `LumenPreviewContext`, `LumenPreviewSession` and the types of their state, errors, options and callbacks, including `MediaFailure` and `LumenPreviewMediaState`. Nothing else is public: media decoding and audio are internal.

The preview object from `createLumenPreview()` is read-only for an app: it has `subscribe`, `getSnapshot`, `play`, `pause`, `seek` and `whenReady`, and only the session mounted with it changes its state. `getSnapshot()` returns the same object until the state changes, then a new one, so it works as a `useSyncExternalStore` snapshot.

`@lumiscia/lumen-preview/advanced` exports only types: the bindings a custom host passes to `@lumiscia/lumen-preview/worker-host` (`LumenBindings`, `LumenPreviewBindings`, `LumenPreviewControllerBinding`, `LumenMediaTarget`) and the types its messages carry (`VideoSourceRegistration`, `PreviewClock`). `@lumiscia/lumen-bindings` implements them, so an application doesn't need this entry.

The preview draws the 3D scene nodes too. The [3D guide](https://github.com/lumiscia/lumen-oss/blob/main/docs/3d.md) covers them.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
