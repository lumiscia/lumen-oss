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

Pass `media` and `fonts` (name to URL) when the composition uses them. `media` also takes the binary glTF (`.glb`) files `model_3d` nodes read, under the name in their `source`; they load with the composition, as images do. The preview renders in a worker; `@lumiscia/lumen-bindings/urls` names that worker and the wasm it loads, and a page that serves copies of them elsewhere passes those URLs instead. Either can be on another origin that sends CORS headers. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`. With Vite 6 or 7, the `@lumiscia/lumen-bindings` README has the dev server setting these URLs need.

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

A media source that can't be used (an image, video, audio, font, model or environment map that is missing, of another kind, unsupported, too large or corrupt) is reported the same way for every kind. `media` on the error says which source, what was wanted, why and where the composition uses it, and its `code` is one of `media.not_found`, `media.wrong_kind`, `media.unsupported`, `media.too_large` or `media.decode`. It is `null` for any other failure:

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

Diagnostic codes are public API, and new ones can appear in a patch release, so handle codes you don't know by their `severity`. The [engine README](https://github.com/lumiscia/lumen-oss/tree/main/crates/engine#diagnostics) lists them.

## Playback controls

`mountPreview` takes these options, and `session.update()` changes them while it plays, without loading or restarting anything. A frame is a frame of the composition's timeline, as in `preview.seek(frame)`.

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

session.update({ composition, range: { in: 30, out: 90 }, loop: true, rate: 1.5, muted: true });
session.rate; // 1.5: the options in effect are readable as loop, rate, volume, muted and range
```

`update()` replaces every input, so pass the ones that should stay. Some details:

- A value that can't be used (a `rate` outside `0.25` to `4`, a `volume` outside `0` to `1`, a `range` that isn't whole frames with `0 <= in < out`, or any of them not finite) is reported through `onError` with the code `"invalid_option"` and the state's `error`. The whole call is rejected: none of its inputs (composition, media, fonts, callbacks or other options) are applied, and the earlier ones stay in effect, also while the worker is still starting. Options that are invalid when the session is created are reported the same way and start from their defaults, with the other inputs used as given.
- A `range` with `out` past the end of the composition is cut to the end, with a console warning. An `in` past the end is ignored with a warning, which plays the whole composition. The range is checked again when a different composition loads.
- Seeks land inside the range, whether or not it loops, and a playhead outside a new range moves to its `in`. Clearing the range restores the whole timeline without moving the playhead. Changing `loop` or `range` keeps the position playback had reached, also when a looping playback has already wrapped round and when the audio has stopped arriving.
- Playback that reached the end of the range stops on its last frame and calls `onEnded` once. A pause, a seek or a change of `rate`, `loop` or `range` just before the end doesn't. Playing from the last frame of a range that doesn't loop starts it from `in` again.
- Changing `loop` or `range` while audio plays restarts the audio mix at the current position, which can leave a brief gap in the sound. Changing `rate`, `volume` and `muted` doesn't.
- Lookahead decoding stays inside the range, and wraps round it only when it loops.

## Entry points

`@lumiscia/lumen-preview` exports `createLumenPreview`, `mountPreview`, `LumenPreviewContext`, `LumenPreviewSession` and the types of their state, errors, options and callbacks, including `MediaFailure`. Nothing else is public: media decoding and audio are internal.

`@lumiscia/lumen-preview/advanced` exports only types: the bindings a custom host passes to `@lumiscia/lumen-preview/worker-host` (`LumenBindings`, `LumenPreviewBindings`, `LumenPreviewControllerBinding`, `LumenMediaTarget`) and the types its messages carry (`MediaRegistration`, `RegisteredImageSource`, `RegisteredModelSource`, `RegisteredVideoSource`, `VideoSourceRegistration`, `PreviewClock`). `@lumiscia/lumen-bindings` implements them, so an application doesn't need this entry.

The preview draws the 3D scene nodes too. The [3D guide](https://github.com/lumiscia/lumen-oss/blob/main/docs/3d.md) covers them.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
