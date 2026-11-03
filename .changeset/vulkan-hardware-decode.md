---
"@lumiscia/lumen-bindings": minor
---

`LUMEN_HARDWARE_DECODE=1` now decodes on Vulkan in `lumen-local` and `lumen-server` on Linux. H.264, HEVC and AV1 at 8-bit 4:2:0 decode through FFmpeg 8.0 or newer on the renderer's own device, and a compute pass converts them to RGBA without leaving the GPU. Everything else moves to CPU decode with one warning per source: older FFmpeg, VP9, 10-bit, sources with alpha, rotated sources and hardware without the codec. A frame that decodes but can't be imported into the renderer now does the same, where it used to fail the render.

For Rust: `FfmpegResolverOptions` is no longer `Copy` and has a `vulkan_device` field on Linux Vulkan builds. `GpuCompositionRenderer::new_vulkan_decode` creates a renderer on a device with Vulkan decode, and `GpuVideoFrame` has a `Vulkan` variant.
