---
"@lumiscia/lumen-bindings": patch
---

`lumen_gpu::Renderer::read_texture_rgba8` reads an RGBA8 texture back to the CPU with tightly packed rows, so hosts and tests no longer carry their own copy of the padded-row readback. Rendered pixels are unchanged.
