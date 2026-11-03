---
"@lumiscia/lumen-bindings": minor
---

An expression that gives NaN or infinity for an enum param is now an error instead of selecting the first option. A failure to open a video encoder names the encoder, and `lumen-server` keeps up to 1,024 characters of an error message instead of 256, so the codec, stream and node details at the end of it reach the client. `RenderError` no longer has the variants nothing returned (`Cancelled`, `MissingNodeOutput`, `InvalidMediaOutputType`, `SurfaceAllocation`, `PixelReadbackFailed`, `ScratchSurfaceUnavailable`), and `CudaDriver::describe_error` is gone.
