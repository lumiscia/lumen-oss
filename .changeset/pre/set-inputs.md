---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

`LumenPreviewSession.update()` is now `setInputs()`. It still replaces every input, so one left out returns to its default, and the name says so: `session.setInputs({ muted: true })` clears the composition, which `update` suggested it wouldn't. Unchanged inputs do nothing. It reads the composition again on every call, so one edited in place is picked up, and it costs a serialization: call it when an input may have changed rather than on every frame.

```ts
// Before
session.update({ composition, rate: 2 });

// After
session.setInputs({ composition, rate: 2 });
```

`Player` in `@lumiscia/lumen-react` and `@lumiscia/lumen-svelte` passes every prop it doesn't use itself on to `setInputs`, so a prop can no longer be missed.
