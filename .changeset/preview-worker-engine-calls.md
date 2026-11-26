---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-bindings": minor
---

`LumenPreviewSession` has `check`, `evaluate` and `mediaDependencies`, which run the bindings' functions of the same names in the preview's worker, so a page that already shows a preview doesn't load a second copy of the wasm on the main thread to call them. Each takes a composition as the session's `composition` input does, with the same other arguments as the bindings' function, and resolves with the same result. They work as soon as the worker is ready, need no composition loaded and leave what the preview shows alone; a call still pending when the session is disposed rejects with an `AbortError`. The 2D build's worker answers them too:

```ts
const session = mountPreview(document.body, { preview, composition, runtime });
const diagnostics = await session.check(draft, ["Inter"]);
const opacity = await session.evaluate(draft, { node: 4, param: "opacity", frame: 30 });
const { media } = await session.mediaDependencies(draft);
```
