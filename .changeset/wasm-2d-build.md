---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

The browser preview ships two builds. The default renders 2D and 3D nodes, and now reads glTF models with Draco meshes and WebP textures as well as meshopt buffers. KTX2 textures stay out of the browser build, because their transcoder would make the wasm about 350 KB larger to download; `lumen` and `lumen-server` still read them.

A page that only shows 2D compositions can load the 2D-only build instead, which has no 3D and is about a quarter smaller to download (1.23 MB against 1.65 MB, brotli-compressed). It reports each 3D node as a `node.unsupported` error. Import `Player` from the `/2d` entry point:

```tsx
import { Player, createLumenPreview } from "@lumiscia/lumen-react/2d";
// or, in Svelte: import { Player, createLumenPreview } from "@lumiscia/lumen-svelte/2d";
```

With `mountPreview`, take both URLs from `@lumiscia/lumen-bindings/urls-2d`. The 2D wasm runs only in its own worker, `worker-2d.js`:

```ts
import { mountPreview } from "@lumiscia/lumen-preview";
import { wasmUrl, workerUrl } from "@lumiscia/lumen-bindings/urls-2d";

mountPreview(host, { preview, composition, wasmUrl, workerUrl });
```

A bundler emits only the build a page imports. With `runtimeUrl`, the `/2d` `Player` loads `worker-2d.js` and `lumen_wasm_2d_bg.wasm` from that directory, and `runtimeFileUrl(runtimeUrl, name)` from `@lumiscia/lumen-preview` gives a file's URL there for pages that call `mountPreview` themselves.
