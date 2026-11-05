# @lumiscia/lumen-svelte

The Lumen `Player` component for previewing a composition in Svelte 5. It needs a browser with WebGPU.

```bash
pnpm add @lumiscia/lumen-svelte @lumiscia/lumen-bindings @lumiscia/lumen-shared
```

```svelte
<script lang="ts">
  import { Composition } from "@lumiscia/lumen-shared";
  import { Player, createLumenPreview } from "@lumiscia/lumen-svelte";
  import { createLumenBindings } from "@lumiscia/lumen-bindings/bundler";

  const composition = new Composition({ timeline: { durationSeconds: 3 } });
  const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
  const title = composition.addNode({
    type: "text",
    params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
  });
  const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
  composition.addNode({ type: "media_output", inputs: { source: merged } });

  const preview = createLumenPreview();
  const bindings = createLumenBindings();
</script>

<Player {preview} {bindings} {composition} style="width: 640px" />
<button disabled={!preview.state.isLoaded} onclick={() => preview.play()}>Play</button>
```

Pass `media` and `fonts` (name to URL) to `Player` when the composition uses them. `examples/vite-svelte` is a complete app.

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
