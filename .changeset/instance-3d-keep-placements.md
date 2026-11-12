---
"@lumiscia/lumen-bindings": patch
---

An `instance_3d` whose params don't read the frame places its copies once and keeps their matrices, so a later frame only places them under what holds the node: a still field of copies under a moving camera, light or parent no longer evaluates `position`, `rotation` and `scale` for every copy on every frame. The kept matrices are dropped when the composition changes, or after a frame that doesn't draw the node. Nothing renders differently.
