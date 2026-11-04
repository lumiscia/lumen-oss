---
"@lumiscia/lumen-bindings": minor
---

Vulkan hardware decode no longer exhausts the file limit on footage that seeks: on NVIDIA, a job on seeking H.264 held about 1,390 file descriptors and now holds about the same as one that doesn't seek. A source with a GPU frame already on a texture no longer moves to the CPU mid-render, which changed its pixels from that frame on; the render now fails with an error that says so. `lumen-server` and `lumen-local` raise their open file limit to the hard limit at startup.
