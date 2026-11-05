---
"@lumiscia/lumen-bindings": patch
---

`h264_nvenc` on a machine with no CUDA library now falls back to CPU upload instead of panicking. `CudaDriver::load` returns an error when the library isn't installed.
