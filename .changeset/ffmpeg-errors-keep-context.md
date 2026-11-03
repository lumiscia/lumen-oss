---
"@lumiscia/lumen-bindings": patch
---

FFmpeg error messages now name the GPU backend, codec and stream they came from. CUDA copy and encode failures on `lumen-server` name the frame, and a video encoder that fails to open is reported as not retryable.
