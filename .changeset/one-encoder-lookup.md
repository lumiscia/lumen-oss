---
"@lumiscia/lumen-bindings": minor
---

`lumen-local` and `lumen-server` now resolve an encoder name through FFmpeg, so `hevc_nvenc` and `av1_nvenc` work in `lumen-local`, `libsvtav1` and other installed encoders work in `lumen-server`, and an unknown name fails before rendering starts. The `h264`, `hevc`, `h265` and `av1` aliases and the `*_vaapi` encoders, which never rendered, are now refused with a clear error. In `lumen_ffmpeg`, `VideoEncoderConfig` takes one `encoder` name and one `quality`, every video frame needs a `pts`, and `gpu_texture_encode_support` is gone in favour of `encoder_info`.
