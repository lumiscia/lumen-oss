---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

Renders pick their quality by preset and their codec by name. Every host defines `draft`, `standard`, `high` and `lossless`, each with settings per FFmpeg encoder, and `render_settings` in a composition gains `codec` (`h264`, `hevc` or `av1`) and `quality` (a preset name), which the `RenderSettings` type and `new Composition({ renderSettings: { codec, quality } })` carry. Templates' render settings are now `TemplateRenderSettings`, without them. Previews ignore both.

A render with no preset now uses `standard` instead of a 14 Mb/s target. Each codec tries its encoders in order, with one warning for each that won't open: on Linux the Vulkan encoder, then `libx264`, `libx265` or `libsvtav1`, and on macOS VideoToolbox. A preset only tries the encoders it has settings for, so `lossless` uses `libx264`, `libx265` or NVENC H.264 and HEVC. `LUMEN_VIDEO_ENCODER` still pins one encoder with no fallback. An output container that can't hold the codec, such as H.264 in `.webm`, fails when the encoder is created.

`LUMEN_QUALITY_CONFIG` (and `lumen-server --quality-config`) names a JSON file that sets the default preset and codec, each codec's encoder order, and presets' settings per encoder: `crf`, `qp` or `bitrate`, an encoder `preset`, and raw FFmpeg `options`. It is checked when `lumen-server` starts or `lumen-local` loads it, so an unknown field, codec, encoder or option, or a value outside an option's range, fails then instead of on a render.

`lumen-local` gains `--quality`, `--codec` and `--quality-config`. `--bitrate`, `--crf`, `--qp` and `--preset` keep their meaning, replacing the preset on every encoder tried, and can't be combined with `--quality`. `--crf`, `--qp` and `--preset` now also work on `libx265` and `libsvtav1`.

For Rust users of `lumen-engine-ffmpeg`, `QualityConfig` loads and resolves presets into a `VideoEncoderChoice`, which holds the ordered `EncoderCandidate`s a render tries; `EncoderQuality` gains raw `options`, and its `rate_control` is optional, so `EncoderQuality::default()` and `VideoEncoderConfig::cpu_rgba` leave rate control at the encoder's default. `VideoEncoderChoice::resolve` and `check_quality` are gone. In `lumen-server`, `RenderOptions::encoder` and `resolve_encoder` replace `video_encoder` and `quality`, and `Mp4Render::open` takes the choice.
