---
"@lumiscia/lumen-bindings": minor
---

Node structs now derive `Default` instead of spelling it out, so a new node cannot get its default wrong. `PortRef::empty()` is replaced by `PortRef::default()`.
