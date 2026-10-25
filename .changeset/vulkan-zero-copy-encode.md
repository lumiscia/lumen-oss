---
"@lumiscia/lumen-bindings": minor
---

On Linux, `lumen-local --encoder av1_vulkan` (or `h264_vulkan`/`hevc_vulkan`) now renders and encodes on one shared Vulkan device, converting frames to NV12 on the GPU instead of reading them back. It falls back to the CPU upload path when the device has no Vulkan Video encode support or separate compute queue family, or when the frame size is odd. The new `VulkanDevice`, `VulkanRenderQueue` and `VideoEncoderConfig::vulkan_device` back `EncodeMode::GpuTexture(GpuBackend::Vulkan)`.
