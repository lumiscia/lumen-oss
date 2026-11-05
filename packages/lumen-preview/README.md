# @lumiscia/lumen-preview

The framework-independent browser preview for Lumen: playback state, a transport, media decoding and audio. Use it to preview in plain JavaScript, or through `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte`. It needs a browser with WebGPU and `@lumiscia/lumen-bindings`.

```bash
pnpm add @lumiscia/lumen-preview @lumiscia/lumen-bindings @lumiscia/lumen-shared
```

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

Pass `media` and `fonts` (name to URL) when the composition uses them.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
