---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

Shapes and paths take a `repeat` param that draws up to 1024 copies of the node in one pass. Copy `i` adds `i` times `rotation_offset`, `scale_offset` and `opacity_offset` to the node's own rotation, scale and opacity, about its own anchor, and moves by `position_offset` per column. With `columns` set, copies wrap into rows that move by `row_offset`. Each offset takes expressions, evaluated once per frame. Each copy's opacity is clamped to 0–1, later copies draw over earlier ones, and the node's texture covers only the copies that draw anything.
