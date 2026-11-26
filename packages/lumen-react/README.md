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

`useLumenPreview(preview)` re-renders on every played frame, because the state's `frame` changes. A component that reads one part of the state can pass a selector, such as `useLumenPreview(preview, (state) => state.isPlaying)`, and then re-renders only when that value changes. The selector must return a field or a primitive: one that builds a new object or array on each call makes React throw "Maximum update depth exceeded".

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

`Player` from `@lumiscia/lumen-react` loads the wasm that renders 2D and 3D nodes. A page that only shows 2D compositions can import `Player` from `@lumiscia/lumen-react/2d` instead: its wasm has no 3D and is about a quarter smaller to download, and it reports each 3D node as a `node.unsupported` error. Its runtime is `runtime2d` from `@lumiscia/lumen-bindings/urls-2d`, whose wasm runs only in its own worker. The rest of the package is the same from either entry point.

```tsx
import { Player, createLumenPreview } from "@lumiscia/lumen-react/2d";
```

The preview renders in a worker that loads the wasm renderer, and plays audio through its own worker and an audio worklet. All four files come from `@lumiscia/lumen-bindings`, a dependency of this package, and Vite or plain ESM finds them next to it. They can be on another origin, such as a CDN, that sends CORS headers. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`.

The reliable way to host these files with any bundler or CDN is `runtimeUrl`: copy the four files the `Player`'s manifest names (`runtime` from `@lumiscia/lumen-bindings/urls`, or `runtime2d` from `/urls-2d` for the `/2d` `Player`) into one directory, which the [`@lumiscia/lumen-bindings` README](https://github.com/lumiscia/lumen-oss/tree/main/packages/lumen-bindings#runtime-files) lists. With it, `Player` loads every file from that directory and needs none of the Vite settings below.

```jsx
<Player preview={preview} composition={composition} runtimeUrl="/lumen-runtime/" />
```

To serve one file somewhere else, pass `runtime`, a manifest of your own, such as `{ ...runtime, wasm: { file: "lumen_wasm_bg.wasm", url: wasmUrl } }`.

With Vite 6 or 7, keep `@lumiscia/lumen-bindings` out of the dev server's dependency pre-bundling. It moves the manifest that locates the runtime files into `node_modules/.vite/deps`, so they 404. Vite 8 and `vite build` need nothing.

```js
// vite.config.js
export default defineConfig({
  optimizeDeps: { exclude: ["@lumiscia/lumen-bindings"] },
});
```

`onError` on `Player` is called with each failure the state's `error` shows. When the engine rejects the composition, its `diagnostics` say what is wrong and where, each with a `severity`, a public `code`, a JSON pointer `path` and a `message`. New codes can appear in a patch release, so handle codes you don't know by their `severity`. The [engine README](https://github.com/lumiscia/lumen-oss/tree/main/crates/engine#diagnostics) lists them.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
