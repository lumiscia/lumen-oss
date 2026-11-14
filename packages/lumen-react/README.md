# @lumiscia/lumen-react

The Lumen `Player` component and hooks for previewing a composition in React. It needs React 18 or newer and a browser with WebGPU.

```bash
pnpm add @lumiscia/lumen-react @lumiscia/lumen-shared
```

```tsx
import { Composition } from "@lumiscia/lumen-shared";
import { Player, createLumenPreview, useLumenPreview } from "@lumiscia/lumen-react";

const composition = new Composition({ timeline: { durationSeconds: 3 } });
const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
const title = composition.addNode({
  type: "text",
  params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
});
const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
composition.addNode({ type: "media_output", inputs: { source: merged } });

const preview = createLumenPreview();

export const App = () => {
  const state = useLumenPreview(preview);
  return (
    <>
      <Player preview={preview} composition={composition} style={{ width: 640 }} />
      <button disabled={!state.isLoaded} onClick={() => preview.play()}>
        Play
      </button>
    </>
  );
};
```

`Player` also takes `loop`, `rate` (0.25 to 4), `volume` (0 to 1), `muted` and `range` (`{ in, out }` in frames, `out` exclusive). They change live, without a remount or a restart of playback. `onEnded` is called once when playback reaches the end of the range without `loop`, and `onTimeUpdate` with `{ frame, timeMs }` for each frame shown; neither re-renders the component, unlike `useLumenPreview`.

```tsx
<Player
  preview={preview}
  composition={composition}
  loop={loop}
  rate={rate}
  volume={volume}
  muted={muted}
  range={{ in: 30, out: 90 }}
  onEnded={() => setPlaying(false)}
  onTimeUpdate={({ frame }) => scrubberRef.current?.setAttribute("value", String(frame))}
/>
```

[`@lumiscia/lumen-preview`](https://github.com/lumiscia/lumen-oss/tree/main/packages/lumen-preview#playback-controls) lists what each does and what is rejected.

Pass `media` and `fonts` (name to URL) to `Player` when the composition uses them. `examples/vite-react` is a complete app.

`Player` from `@lumiscia/lumen-react` loads the wasm that renders 2D and 3D nodes. A page that only shows 2D compositions can import `Player` from `@lumiscia/lumen-react/2d` instead: its wasm has no 3D and is about a quarter smaller to download, and it reports each 3D node as a `node.unsupported` error. It runs in its own worker, `@lumiscia/lumen-bindings/worker-2d`, so a page that passes `wasmUrl` to this `Player` passes `workerUrl` too. The rest of the package is the same from either entry point.

```tsx
import { Player, createLumenPreview } from "@lumiscia/lumen-react/2d";
```

The preview renders in a worker, which loads the wasm renderer. Both come from `@lumiscia/lumen-bindings`, a dependency of this package, and Vite or plain ESM finds them next to it. They can be on another origin, such as a CDN, that sends CORS headers. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`. A page that serves them elsewhere passes `workerUrl` and `wasmUrl` to `Player`, pointing at `@lumiscia/lumen-bindings/worker` and `@lumiscia/lumen-bindings/wasm`. With a bundler, its asset imports give those URLs:

```js
import wasmUrl from "@lumiscia/lumen-bindings/wasm?url";
import workerUrl from "@lumiscia/lumen-bindings/worker?url";
```

The reliable way to host these files with any bundler or CDN is `runtimeUrl`: a directory serving `worker.js` and `lumen_wasm_bg.wasm` (`worker-2d.js` and `lumen_wasm_2d_bg.wasm` for the `/2d` `Player`), `audio-worker.js` and `audio-worklet.js`, which the `@lumiscia/lumen-preview` README lists. With it, `Player` derives every runtime URL from that directory and needs none of the Vite settings below.

```jsx
<Player preview={preview} composition={composition} runtimeUrl="/lumen-runtime/" />
```

With Vite 6 or 7, keep `@lumiscia/lumen-bindings` and `@lumiscia/lumen-preview` out of the dev server's dependency pre-bundling. It moves the modules that locate the worker, wasm and audio worker into `node_modules/.vite/deps`, so they 404. Vite 8 and `vite build` need nothing.

```js
// vite.config.js
export default defineConfig({
  optimizeDeps: { exclude: ["@lumiscia/lumen-bindings", "@lumiscia/lumen-preview"] },
});
```

`onError` on `Player` is called with each failure the state's `error` shows. When the engine rejects the composition, its `diagnostics` say what is wrong and where, each with a `severity`, a public `code`, a JSON pointer `path` and a `message`. New codes can appear in a patch release, so handle codes you don't know by their `severity`. The [engine README](https://github.com/lumiscia/lumen-oss/tree/main/crates/engine#diagnostics) lists them.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
