---
"@lumiscia/lumen-bindings": patch
---

Renders request the GPU's real device limits instead of wgpu's conservative defaults, so canvases and images larger than 8192 pixels work on GPUs that support them.
