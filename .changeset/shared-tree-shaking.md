---
"@lumiscia/lumen-types": patch
"@lumiscia/lumen-shared": patch
"@lumiscia/lumen-sdk": patch
"@lumiscia/lumen-react": patch
---

Bundles that use the composition builder are much smaller. `@lumiscia/lumen-shared` no longer pulls in the documentation of every expression builtin, and the four packages declare `"sideEffects": false`, so a bundler can drop the node tables nothing imports. A bundle of `Composition`, `nodes` and `frame` went from 185,135 to 14,397 bytes minified with esbuild.
