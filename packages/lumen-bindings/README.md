# @lumiscia/lumen-bindings

The Lumen preview renderer compiled to WebAssembly. The preview packages take it as their `bindings`. Previewing needs a browser with WebGPU.

```bash
pnpm add @lumiscia/lumen-bindings @lumiscia/lumen-preview @lumiscia/lumen-shared
```

Entry points: `/bundler` for Vite and other bundlers, `/browser` (also the default) when you serve the files yourself, and `/node`, which parses compositions but can't render.

```ts
import { createLumenPreview, mountPreview } from "@lumiscia/lumen-preview";
import { createLumenBindings } from "@lumiscia/lumen-bindings/bundler";
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
mountPreview(document.body, {
  preview,
  bindings: createLumenBindings(),
  composition,
});

// Loading is asynchronous, and play() before it finishes is ignored.
document.querySelector("button")?.addEventListener("click", () => {
  if (preview.getSnapshot().isLoaded) preview.play();
});
```

Under Vite, `/bundler` works without extra plugins; `examples/vite-react` and `examples/vite-svelte` use it with a plain Vite config. In React or Svelte, use `Player` from `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte` instead of `mountPreview`.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
