---
"@lumiscia/lumen-bindings": patch
---

Zero-copy `h264_vulkan` renders on NVIDIA run 19-32% faster on stock FFmpeg 8.0. The shared Vulkan device now lists one video encode queue, which also stops the encoder stalling at GOP boundaries, so the encoder no longer needs to be limited to one frame in flight. `h264_vulkan` encodes of CPU frames still use one frame in flight.
