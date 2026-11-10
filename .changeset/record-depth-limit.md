---
"@lumiscia/lumen-bindings": patch
---

A composition nested more than 48 nodes deep now fails with a `render.too_deeply_nested` error that names the node, instead of crashing the process with a stack overflow.
