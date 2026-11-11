# @lumiscia/lumen-svelte

The Lumen `Player` component for previewing a composition in Svelte 5. It needs a browser with WebGPU.

```bash
pnpm add @lumiscia/lumen-svelte @lumiscia/lumen-shared
```

```svelte
<script lang="ts">
  import { Composition } from "@lumiscia/lumen-shared";
  import { Player, createLumenPreview } from "@lumiscia/lumen-svelte";

  const composition = new Composition({ timeline: { durationSeconds: 3 } });
  const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
  const title = composition.addNode({
    type: "text",
    params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
  });
  const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
  composition.addNode({ type: "media_output", inputs: { source: merged } });

  const preview = createLumenPreview();
</script>

<Player {preview} {composition} style="width: 640px" />
<button disabled={!preview.state.isLoaded} onclick={() => preview.play()}>Play</button>
```

`Player` also takes `loop`, `rate` (0.25 to 4), `volume` (0 to 1), `muted` and `range` (`{ in, out }` in frames, `out` exclusive). They change live, without a remount or a restart of playback. `onEnded` is called once when playback reaches the end of the range without `loop`, and `onTimeUpdate` with `{ frame, timeMs }` for each frame shown; neither touches `preview.state`.

```svelte
<Player
  {preview}
  {composition}
  {loop}
  {rate}
  {volume}
  {muted}
  range={{ in: 30, out: 90 }}
  onEnded={() => (playing = false)}
  onTimeUpdate={({ frame }) => (scrubber = frame)}
/>
```

[`@lumiscia/lumen-preview`](https://github.com/lumiscia/lumen-oss/tree/main/packages/lumen-preview#playback-controls) lists what each does and what is rejected.

Pass `media` and `fonts` (name to URL) to `Player` when the composition uses them. `examples/vite-svelte` is a complete app.

The preview renders in a worker, which loads the wasm renderer. Both come from `@lumiscia/lumen-bindings`, a dependency of this package, and Vite or plain ESM finds them next to it. They can be on another origin, such as a CDN, that sends CORS headers. A worker on another origin starts from a `blob:` URL that imports it, so a Content Security Policy must allow `blob:` and that origin in `worker-src`. A page that serves them elsewhere passes `workerUrl` and `wasmUrl` to `Player`, pointing at `@lumiscia/lumen-bindings/worker` and `@lumiscia/lumen-bindings/wasm`. With a bundler, its asset imports give those URLs:

```js
import wasmUrl from "@lumiscia/lumen-bindings/wasm?url";
import workerUrl from "@lumiscia/lumen-bindings/worker?url";
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
