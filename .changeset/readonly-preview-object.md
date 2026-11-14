---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

The object from `createLumenPreview()` is read-only for an app. It has `subscribe`, `getSnapshot`, `play`, `pause`, `seek` and `whenReady`; `update()`, `attach()` and `detach()` are gone, and `LumenPreviewPatch` and `LumenPreviewTransport` are no longer exported. Only the session mounted with the preview changes its state.

`getSnapshot()` returns a read-only `LumenPreviewState`: the same object until the state changes, then a new one. Read it, or pass `preview.subscribe` and `preview.getSnapshot` to `useSyncExternalStore`, instead of changing it.

```ts
// Before
preview.update({ isLoaded: true }); // set the state by hand

// After: the state follows the session
const preview = createLumenPreview();
mountPreview(host, { preview, composition, wasmUrl, workerUrl });
preview.getSnapshot().isLoaded;
```
