---
"@lumiscia/lumen-bindings": patch
---

Hardware-decoded video converts to RGBA with the same YUV shader as every other decode path, following the stream's colour tags. VideoToolbox frames on macOS no longer render white, and rotated sources now decode on the GPU and show upright.
