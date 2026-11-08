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

Pass `media` and `fonts` (name to URL) to `Player` when the composition uses them. `examples/vite-react` is a complete app.

The preview renders in a worker, which loads the wasm renderer. Both come from `@lumiscia/lumen-bindings`, a dependency of this package, and Vite or plain ESM finds them next to it. They can be on another origin, such as a CDN, that sends CORS headers. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`. A page that serves them elsewhere passes `workerUrl` and `wasmUrl` to `Player`, pointing at copies of `dist/browser/worker.js` and `wasm/lumen_wasm_bg.wasm` from that package.

With Vite 6 or 7, keep `@lumiscia/lumen-bindings` and `@lumiscia/lumen-preview` out of the dev server's dependency pre-bundling. It moves the modules that locate the worker, wasm and audio worker into `node_modules/.vite/deps`, so they 404. Vite 8 and `vite build` need nothing.

```js
// vite.config.js
export default defineConfig({
  optimizeDeps: { exclude: ["@lumiscia/lumen-bindings", "@lumiscia/lumen-preview"] },
});
```

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
