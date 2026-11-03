---
"@lumiscia/lumen-bindings": minor
---

A node's input fields are now the only record of its connections, so the checks that run before rendering read the same wiring the renderer follows. A second connection to an input that takes one source is now an error, ``input `base` of node 3 is already connected``, instead of replacing the first. Inputs that take several, such as `layers`, still append. JSON `connections` are unchanged.

For Rust: `Graph` holds only `nodes`; `connections` and `outgoing_connection_count` are gone, and `Graph::connect` writes the connection into the target node.
