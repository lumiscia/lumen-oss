---
"@lumiscia/lumen-bindings": minor
---

For Rust users of `lumen-engine-ffmpeg`: the GPU encode telemetry types (`GpuEncodeTelemetry`, `GpuEncodeEvent`, `GpuUploadDescriptor` and `MuxedEncoder::gpu_telemetry`) are removed, since nothing read them and their upload timings were measured at the wrong point. `VideoEncoder`, `AudioEncoder` and `OutputContext` are no longer public, because `MuxedEncoder` is the only way to finish an encode, and `lumen_ffmpeg::encode::RateControl` is now `lumen_ffmpeg::RateControl`. `AudioFrame` and `AudioResamplerConfig` lose `sample_format` and `pts`, and `SampleFormat` is removed: audio is always interleaved f32 and is written as one contiguous stream from time zero. `MediaFrame::ExternalTexture` and its frame and handle types are gone from `lumen-engine`, and the `avcodec`, `avformat`, `swresample` and `swscale` features of `lumen-engine-ffmpeg` are always on.
