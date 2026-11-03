---
"@lumiscia/lumen-bindings": minor
---

Removed the `channel_shuffle` node. It could write a channel straight into a premultiplied raster, such as alpha forced to one under unchanged colour, which no other node can do. A composition that still uses it is rejected with a message; a `wgsl_shader` node rearranges channels when that is needed.
