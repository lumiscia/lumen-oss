---
"@lumiscia/lumen-bindings": minor
---

A connection's source is now just the node it comes from, since every node has one output. JSON `connections` read as before, and `from_port` may still be written but must be `output`; anything else is an error naming the connection. The composition schema and SDK types do not change. A node that is missing a required input, such as a `blur` with nothing connected to `source`, now fails with ``node 3 is missing required input `source` `` when it compiles as well as when it validates.

For Rust: node inputs are `Option<NodeId>`, or `Vec<NodeId>` for variadic inputs. `PortRef`, `OutputPortDef`, `PortKind`, `SINGLE_RASTER_OUTPUT` and the `port` argument of `compile_gpu` are removed, and `Connection` loses `from_port`.
