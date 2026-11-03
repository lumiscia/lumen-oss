---
"@lumiscia/lumen-bindings": patch
---

Each node kind in `lumen-engine` is now declared on one line of a table in `node/kind.rs`, which generates the `NodeKind` enum, its dispatch, its schema list and JSON construction. For Rust: `NodeKind::id`, `input_ports` and `has_expressions` are now reached through the `Node` trait, which gains `has_expressions`, and `JsonNode` gains a `KIND` constant. The generated schema and composition JSON are unchanged.
