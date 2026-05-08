# Lumen TypeScript

TypeScript packages for building, previewing, and rendering Lumen compositions.

This workspace contains the schema-derived public types, a small composition builder, the Node SDK client, browser preview bindings, and React/Svelte canvas wrappers. The generated SDK types are sourced from `vendor/lumen-definitions` so application code and rendering infrastructure stay aligned with the Lumen composition schema.

## Packages

| Package | Purpose |
| --- | --- |
| `@lumiscia/lumen-types` | Generated TypeScript types for Lumen schemas and node metadata. |
| `@lumiscia/lumen-shared` | Dependency-light shared helpers, including the `Composition` builder. |
| `@lumiscia/lumen-sdk` | Node SDK for media upload, render creation, render polling, artifacts, and render events. |
| `lumen-bindings` | WASM binding package for browser, bundler, Node, and no-module targets. |
| `lumen-preview` | Browser preview engine used by framework wrappers. |
| `@lumiscia/lumen-react` | React preview state and `LumenCanvas` component. |
| `@lumiscia/lumen-svelte` | Svelte preview state and `LumenCanvas` component. |

The examples under `examples/` show the SDK in Node and local preview in Vite React and Vite Svelte apps.

## Install

This repository uses pnpm workspaces.

```sh
pnpm install
```

If you pull changes that modify `package.json`, `pnpm-lock.yaml`, generated packages, or WASM binding metadata, run `pnpm install` before continuing.

## Common Commands

```sh
pnpm generate:types
pnpm check
pnpm build
```

Use `pnpm generate:types` after changing `vendor/lumen-definitions` or the type generator. Generated files live in `packages/lumen-types/src/generated/` and should not be hand-edited.

To refresh the checked-in WASM bindings metadata and package files:

```sh
pnpm download:wasm
```

## Build a Composition

`@lumiscia/lumen-shared` provides a typed builder for Lumen composition JSON:

```ts
import { Composition } from "@lumiscia/lumen-shared";

const composition = new Composition({
  metadata: {
    name: "Hello Lumen",
  },
  renderSettings: {
    width: 1920,
    height: 1080,
    background_color: [12, 12, 16, 255],
  },
  timeline: {
    fps: 24,
    durationSeconds: 5,
  },
});

const background = composition.addSolidColor({
  width: 1920,
  height: 1080,
  color: [24, 28, 36, 255],
});

const title = composition.addText({
  content: "Hello from Lumen",
  fontFamily: "Inter",
  fontSize: 96,
  fontWeight: 700,
  color: [255, 255, 255, 255],
  maxWidth: 1280,
});

const merge = composition.addNode({
  type: "merge",
  properties: {
    opacity: 1,
    blend_mode: 0,
  },
});

composition.connect(background, merge, { toPort: "base" });
composition.connect(title, merge, { toPort: "overlay" });
composition.addOutput(merge);

const json = composition.toJSON();
```

## Render from Node

`@lumiscia/lumen-sdk` includes the composition builder and the API client:

```ts
import { Composition, Lumen, mediaReference } from "@lumiscia/lumen-sdk";

const lumen = new Lumen({
  apiKey: process.env.LUMEN_API_KEY ?? "",
});

const image = await lumen.createUrlMedia({
  fileName: "plate.png",
  url: "https://example.com/plate.png",
});

const composition = new Composition();
const source = composition.addImage("plate");
composition.addOutput(source);

const result = await lumen.render(composition, {
  media: {
    plate: mediaReference(image),
  },
});

if (result.error) {
  throw result.error;
}

console.log("Created render:", result.id);
```

Run the Node example with:

```sh
LUMEN_API_KEY=... pnpm --filter @lumiscia/example-node start
```

## Preview in the Browser

Local preview uses `lumen-preview`, `lumen-bindings`, and a framework wrapper.

React:

```tsx
import { LumenCanvas, createLumenPreview } from "@lumiscia/lumen-react";
import * as lumenBindings from "lumen-bindings/bundler";

const preview = createLumenPreview();

export function Preview({ compositionJson }: { compositionJson: string }) {
  return (
    <LumenCanvas
      preview={preview}
      bindings={lumenBindings}
      compositionJson={compositionJson}
    />
  );
}
```

Svelte:

```svelte
<script lang="ts">
  import { LumenCanvas, createLumenPreview } from "@lumiscia/lumen-svelte";
  import * as lumenBindings from "lumen-bindings/bundler";

  const preview = createLumenPreview();
  export let compositionJson: string;
</script>

<LumenCanvas {preview} bindings={lumenBindings} {compositionJson} />
```

Run the browser examples with:

```sh
pnpm --filter @lumiscia/example-vite-react dev
pnpm --filter @lumiscia/example-vite-svelte dev
```

## Definitions and Generated Types

Lumen schema and node metadata are stored under `vendor/lumen-definitions`:

- `schemas/composition.schema.json`
- `schemas/meta.schema.json`
- `meta.json`

The generator in `tooling/generate-types` reads those inputs and writes deterministic TypeScript files to `packages/lumen-types/src/generated/`. If a generated type is wrong, fix the generator or the upstream definition instead of editing generated output directly.

## Development Notes

- Keep public package exports narrow and intentional.
- Prefer exported types for public SDK surfaces.
- Avoid runtime dependencies in type-only packages.
- Keep generated output deterministic so schema updates are easy to review.
- Run `pnpm check` and `pnpm build` before publishing or opening a pull request.
