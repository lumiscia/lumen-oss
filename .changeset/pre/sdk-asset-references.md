---
"@lumiscia/lumen-sdk": minor
---

Reference an uploaded file with `asset:<id>`, which is what `lumen-server` and the hosted API read. `render` no longer turns an upload in `media` into `lumen:<id>`, which `lumen-server` rejected. `mediaReference` and `LumenMediaReference` are removed, and `media` takes only remote `http` or `https` URLs by alias. Write `assetSource(upload)` in the node's source:

```ts
// Before
const clip = await lumen.uploadMedia({ body, fileName: "clip.mp4" });
composition.addNode({ type: "media_in", params: { kind: "video", source: "clip" } });
await lumen.render(composition, { media: { clip } });

// After
const clip = await lumen.uploadMedia({ body, fileName: "clip.mp4" });
composition.addNode({
  type: "media_in",
  params: { kind: "video", source: assetSource(clip) },
});
await lumen.render(composition);
```
