---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

The browser preview loads `.cube` LUTs, and a failed render tells the host the engine's code for it.

- A `color_grade` with `lut_source` works in the preview. Put the `.cube` file in `media` under the name `lut_source` holds, and it loads like a model or an environment map: `pending`, then `ready` or `failed`, counted in `state.media`, awaited by `whenReady()`, fetched abortably and released when the composition stops naming it. It is read in full when it is registered, so a file that isn't a table the engine can use fails the load with the `media.decode`, `media.unsupported` or `media.too_large` a render would give.
- `LumenPreviewMediaKind` gains `"lut"`. An entry whose source is a `File` named `*.cube`, or a string, `URL`, `Request` or `{ kind: "url" }` source whose path ends in `.cube`, is read as a LUT without a `kind`.
- The wasm controller gains `setLut`, `hasLut` and `removeLutSource`, and `LumenMediaTarget` gains them as optional members. `setLut` throws, with `media`, on a file it can't read.
- **A `render_failed` error now carries the engine's code.** Its `diagnostics` hold one `LumenDiagnostic` with that code (`render.gpu`, `render.limit`, `render.node_evaluation` and the others the engine lists), severity `error`, an empty `path` and the message, so `onError` can tell a composition that needs fixing from a GPU fault. It was always empty for a render failure. A render that failed on a media source keeps `diagnostics` empty and says so in `media`. The `render-failed` worker event gains a required `diagnostics` array.
