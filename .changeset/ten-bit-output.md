---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-sdk": minor
---

**Opt-in 10-bit video output.** A render can ask for 10 bits per sample with `hevc` or `av1`: `render_settings.bit_depth`, the request's `bit_depth`, `lumen-local --bit-depth 10`, the SDK's `render(..., { bitDepth: 10 })`, or a custom preset's `bit_depth` in the quality config. Each wins over the next as `codec` does, and the default stays 8 everywhere.

```json
{ "render_settings": { "width": 1920, "height": 1080, "codec": "hevc", "bit_depth": 10 } }
```

The renderer quantises its float working space straight to 10-bit BT.709 limited range (Y 64 to 940, C 64 to 960), with the same ordered dither as 8-bit. `libx265` and `libsvtav1` encode `yuv420p10le` (HEVC Main 10, AV1 Main); `hevc_vulkan`, `av1_vulkan` and NVENC take P010 where the device supports it. H.264 is always 8-bit and VideoToolbox has no 10-bit path, so asking either for 10 bits is an error. An encoder that can't write 10 bits is skipped like one that won't open, and a pinned one fails the render: a 10-bit request never comes out at 8.

**CPU encoders now get dithered YUV from the GPU.** Every encoder that takes CPU frames, at either depth, receives YUV 4:2:0 converted on the GPU from float colour and quantised once, so 8-bit CPU output is dithered in its final domain too and no longer passes through swscale. Its chroma is the 2x2 average of encoded colour, as the Vulkan and NVENC paths already were, so 8-bit `libx264`, `libx265` and `libsvtav1` output changes slightly.

**Rust API:** `VulkanVideoTarget::nv12(renderer, size, dither)` is `VulkanVideoTarget::new(renderer, size, BitDepth, dither)`, and `CudaNvencTargetPool::new` takes a `BitDepth` before `dither`. `VulkanVideoFrame::new` takes a `VulkanFrameLayout` (`nv12` or `p010`) in place of a width and height. `lumen_engine::gpu::YuvConverter` converts a frame to `Nv12`, `P010`, `Yuv420p` or `Yuv420p10`. In `lumen-ffmpeg`, `VideoEncoderConfig::bit_depth`, `VideoEncoderChoice::with_bit_depth`, `QualityRequest::bit_depth`, `check_bit_depth` and `MuxedEncoder::cpu_input_format` are new, `PixelFormat` has `Yuv420p` and `Yuv420p10`, and `write_video_frame` takes YUV frames in the encoder's own format as well as RGBA8, which a 10-bit encoder refuses.
