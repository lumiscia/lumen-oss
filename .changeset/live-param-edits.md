---
"@lumiscia/lumen-bindings": patch
---

Editing a node's params in place and rendering again with the same renderer now draws the edit instead of reusing the plan compiled before it. Any change to the composition, whether a literal param, an expression source, a connection, the canvas or a template, compiles a fresh plan and drops the cached ones.
