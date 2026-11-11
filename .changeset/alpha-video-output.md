---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-sdk": minor
---

**Video output with an alpha channel.** Two new output formats hold alpha: `mov`, ProRes 4444 from `prores_ks` (`yuva444p10le`) with AAC audio, and `webm`, VP9 from `libvpx-vp9` (`yuva420p`, tagged `alpha_mode=1`) with Opus audio from `libopus`. A render asks for alpha with the request's `alpha`, `lumen-local --alpha` or the SDK's `render(..., { alpha: true })`, which win over `render_settings.alpha`, as `codec` does. The default stays opaque, and an `mp4` is unchanged.

```json
{ "render_settings": { "width": 1920, "height": 1080, "alpha": true } }
```

```json
{ "composition": { "...": "..." }, "alpha": true, "output": { "format": "webm" } }
```

The colour is straight, as a PNG of the frame holds it: the premultiplied linear colour is unpremultiplied in linear light, sRGB-encoded and converted to BT.709 limited range once, from float, with the same ordered dither as opaque output. Alpha is full range and never dithered, and a fully transparent pixel is black with neutral chroma. VP9's 4:2:0 chroma is the alpha-weighted average of each 2x2 block, `Σ aᵢ·cᵢ / Σ aᵢ`, so transparent pixels don't darken edges.

`codec` takes `prores` and `vp9` alongside `h264`, `hevc` and `av1`. A `mov` or `webm` without a codec gets its own; `mp4` holds `h264`, `hevc` and `av1`, `mov` holds `prores` and `webm` holds `vp9`, and anything else is refused with `invalid_output`. ProRes is always 10-bit, VP9 with alpha 8-bit, and either also encodes opaque video. Every built-in preset but `lossless` has `prores_ks` settings (quantiser 8, 4 and 2), and every one has `libvpx-vp9` settings (constant quality, `cpu-used` 0 to 5 as its speed). No hardware encoder writes alpha: auto selection goes straight to the software encoder, and a pinned hardware encoder is refused with `invalid_quality`. Alpha in an `mp4`, or with `h264`, `hevc` or `av1`, is an error, never an opaque video, as is a `prores_ks` option that leaves a profile other than 4444 or, with alpha, an `alpha_bits` other than 16, either of which drops the alpha plane. A composition with audio is refused with `invalid_output` when the render is requested if this FFmpeg has no encoder for the container's audio (`libopus` for WebM, AAC otherwise).

**Breaking, Rust API:** `lumen_server::render::resolve_encoder` takes the `OutputFormat` after the composition and returns a `ServerRenderError` (`invalid_output` or `invalid_quality`) rather than an `anyhow::Error`. `OutputFormat` has `Mov` and `Webm`, `OutputCodec` and `EncodeCodec` have `ProRes` and `Vp9` (so `EncodeCodec::ALL` has five), and `OutputSettings` has `alpha`. In `lumen-ffmpeg`, `VideoEncoderConfig::alpha`, `VideoEncoderChoice::with_alpha`, `QualityRequest::alpha`, `check_alpha` and `check_audio_encoder` are new, `EncoderFamily` has `Vpx` and `ProRes`, `PixelFormat` has `Yuva420p`, `Yuv444p10` and `Yuva444p10`, and `check_bit_depth` refuses 8-bit ProRes. `lumen_engine::gpu::YuvConverter` also converts to `Yuva420p`, `Yuv444p10` and `Yuva444p10`. Code matching these enums exhaustively needs the new arms.

An audio encoder now writes the codec it was opened with, where it was always tagged AAC.
