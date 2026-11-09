---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

Diagnostics reach the TypeScript packages. The wasm bindings add `check(json, fonts?)`, which returns what is wrong with a composition without loading it, and a composition `loadComposition` rejects throws with the same `diagnostics`. `LumenError` in the SDK has `diagnostics`, filled when a render is rejected for an invalid composition. The preview and both `Player` components take `onError`, which receives each failure with its diagnostics. `LumenDiagnostic` is exported from `@lumiscia/lumen-types`.
