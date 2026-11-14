---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

`LumenPreviewSession.update()` is now `setInputs()`. It still replaces every input, so one left out returns to its default, and the name says so: `session.setInputs({ muted: true })` clears the composition, which `update` suggested it wouldn't. Unchanged inputs do nothing, so it is safe to call with the whole set on every render, and the same composition object is read once rather than serialized on every call; pass a new object to change a composition.

```ts
// Before
session.update({ composition, rate: 2 });

// After
session.setInputs({ composition, rate: 2 });
```

`Player` in `@lumiscia/lumen-react` and `@lumiscia/lumen-svelte` passes every prop it doesn't use itself on to `setInputs`, so a prop can no longer be missed.
