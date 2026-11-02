---
"@lumiscia/lumen-bindings": patch
---

Compositions can no longer crash or hang the renderer through expressions or media graphs. Expressions nested more than 64 levels, or functions calling each other more than 64 levels deep, are parse errors instead of a stack overflow. Nodes that read each other's properties or text measurements, directly or in a loop, fail with an error after 8 levels or 10,000 evaluations. `clamp` with a NaN bound is an error instead of a panic, and frame prediction no longer takes exponential time on graphs that share inputs.
