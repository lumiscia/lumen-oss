---
"@lumiscia/lumen-bindings": patch
---

`POST /renders` on `lumen-server` returns `422 invalid_quality` when the encoder named by `LUMEN_VIDEO_ENCODER` can't use the requested quality, such as `crf` or a preset on `h264_vulkan`, instead of accepting the render and failing it later. The automatic default accepts quality its libx264 fallback supports. For Rust users of `lumen-engine-ffmpeg`, `EncoderQuality::check_encoder` and `VideoEncoderChoice::check_quality` run the same check without opening an encoder.
