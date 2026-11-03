---
"@lumiscia/lumen-bindings": patch
---

Fixed `lumen-server` renders on the CUDA/NVENC path (`h264_nvenc`, `hevc_nvenc`, `av1_nvenc`) containing frames from the wrong point in time and differing from run to run. The MP4 container of those renders now says BT.601, the matrix NVENC converts RGB with, instead of BT.709.
