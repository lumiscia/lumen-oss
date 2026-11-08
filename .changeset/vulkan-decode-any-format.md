---
"@lumiscia/lumen-bindings": minor
---

With `LUMEN_HARDWARE_DECODE=1` on Linux, Vulkan hardware decode now covers every H.264, HEVC and AV1 profile the device's driver can decode, not only 8-bit 4:2:0. 10-bit HEVC from iPhones, 10-bit AV1 and 4:2:2 or 4:4:4 HEVC decode on the GPU where the hardware supports them; anything the device refuses still moves to CPU decode with one warning. For Rust users of `lumen-engine-ffmpeg`: `VulkanDecodedFrame::layout` returns a `VulkanFrameLayout` describing the frame's subsampling, depth and buffer layout for `copy_to_buffer`.
