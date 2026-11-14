---
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-cli": minor
"@lumiscia/lumen-bindings": minor
---

The SDK's API types are the generated ones from `@lumiscia/lumen-types`, and the client gains the three routes it lacked.

`Render` is a union on `status`. `waitForRender` returns a `SucceededRender`, so its `url` is a `string`, and a `failed` or `cancelled` render has `error`. A render's `progress` is a number, 0 when the API sends none, and `durationMs` and `outputBytes` are new:

```ts
// Before
const done = await lumen.waitForRender(render);
await fetch(done.url!);

// Now
const done = await lumen.waitForRender(render); // SucceededRender
await fetch(done.url);
if (render.status === "failed") console.error(render.error.code); // FailedRender
```

`TemporaryMedia` is now `Media`, and `parseMedia` rejects a body that lacks one of its fields with `invalid_response`. `RenderFailure.retryable` is always set. `MediaKind`, `MediaMetadata`, `MediaStatus`, `MediaSource`, `OutputFormat`, `FrameRange` and `RenderOutput` are the generated types, re-exported. A media source that fails a render or a request is on `LumenError.media`.

`check`, `getMedia` and `deleteMedia` are new:

```ts
const report = await lumen.check(composition, { media }); // CheckReport: { ok, diagnostics }
const upload = await lumen.getMedia("asset:m_…"); // the id may carry `asset:`
await lumen.deleteMedia(upload);
```

`render` and `check` accept a `CompositionSource`: a `LumenComposition`, or any object with `toJSON()`. A `Composition` from another copy of the package works too. `CompositionSource` is exported from `@lumiscia/lumen-types`.

`CheckResponse` is now `CheckReport`, in the `lumen-server-api` crate and the generated types. `@lumiscia/lumen-cli` exports the generated `CheckReport`, `CompositionSource` and `FrameRange` instead of its own copies.
