---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
---

The preview controller no longer has five operations that nothing used: `validateComposition`, `clearMedia`, `clearVideos`, `clearVideoSource` and `hasFont`.

To check a composition without loading it, use `check` on the preview bindings, which returns diagnostics instead of throwing:

```ts
const { check } = await createLumenBindings().preview();
const diagnostics = check(compositionJson);
```

To drop media, remove the sources one at a time with `removeImageSource`, `removeVideoSource`, `removeFontFamily` and the other `remove…` methods, or call `clear()` to drop everything with the composition.
