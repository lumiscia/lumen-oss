---
"@lumiscia/lumen-bindings": minor
---

NVENC on the CUDA zero-copy path now encodes BT.709 like every other encoder: frames are converted to BT.709 NV12 on the GPU before NVENC and tagged `bt709`, instead of being converted by NVENC with BT.601 and tagged `bt470bg`.
