---
"@lumiscia/lumen-bindings": minor
---

Removed CUDA hardware video decode and its `nvcc` build step. `LUMEN_HARDWARE_DECODE=1` decodes on the renderer's Vulkan device (FFmpeg 8.0+), and anything else, including NVDEC-only setups and older FFmpeg, decodes on the CPU with a warning. CUDA/NVENC encode is unchanged, and the `cuda` feature no longer needs the CUDA toolkit to build.
