---
"@lumiscia/lumen-bindings": patch
---

On Linux, `lumen-local` and `lumen-server` now default to `h264_vulkan` and fall back to `libx264` with one warning when it won't open, while an encoder you name fails instead of falling back. `LUMEN_HARDWARE_DECODE=1` now moves a source to CPU decode with one warning when its hardware decoder fails, instead of failing the render, and sources with alpha always decode on the CPU.
