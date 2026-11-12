---
"@lumiscia/lumen-bindings": patch
---

The engine's frame graph dispatches to each node through one `RecordNode` implementation that the node list generates, and keeps what recording holds across frames in separate parts for retained outputs, text, media, runtime shaders and 3D. Nothing renders differently, and performance is unchanged.
