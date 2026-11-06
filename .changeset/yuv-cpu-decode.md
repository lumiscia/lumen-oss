---
"@lumiscia/lumen-bindings": patch
---

Server CPU video decode hands YUV planes and the stream's colour tags to the renderer instead of converting to RGBA in FFmpeg, so server video converts with the same GPU shader as every other path. Video with an alpha layer keeps it as a plane, and rotation is applied by the shader.
