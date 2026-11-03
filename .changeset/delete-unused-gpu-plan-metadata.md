---
"@lumiscia/lumen-bindings": minor
---

Removed render-plan features nothing used from `lumen-engine-gpu`: node ownership and parameter slots, indexed and indirect drawing, scissors, vertex and index buffers, the `GpuAdapterInfo` mirror (use `wgpu::AdapterInfo`, now optional for external devices), `ExportableVulkanTexture::row_pitch` and several discard-result helpers. Rendering output is unchanged.
