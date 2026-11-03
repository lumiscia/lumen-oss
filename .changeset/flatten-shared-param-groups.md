---
"@lumiscia/lumen-bindings": minor
---

For Rust: `path`, `shape` and `text` params group their transform in `placement`, and `path` and `shape` their fill and stroke in `style`, so `params.position` is now `params.placement.position`. Composition JSON and the schema stay flat.
