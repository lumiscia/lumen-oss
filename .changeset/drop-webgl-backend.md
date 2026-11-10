---
"@lumiscia/lumen-bindings": patch
---

The wasm build no longer includes wgpu's WebGL backend, making the preview's wasm smaller. The preview already required WebGPU, so behavior is unchanged.
