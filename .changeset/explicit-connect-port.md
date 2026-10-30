---
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-types": minor
---

`Composition.connect()` takes the target port explicitly instead of defaulting to `input`, which no node has, so a connection without `toPort` no longer builds a composition the engine rejects. Given the target node, `toPort` is typed to its inputs, from the new generated `NodeInputPortName<Kind>` type: `composition.connect(background, merge, { toPort: "base" })` type-checks, and `toPort: "source"` on a `merge` does not. `addNode` now infers the node kind it returns.
