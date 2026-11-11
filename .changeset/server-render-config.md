---
"@lumiscia/lumen-bindings": minor
---

**Breaking (Rust, `lumen-server` library):** `lumen_server::render` reads no environment variables, and its error has a code to match on.

A `RenderConfig` holds what renders take from the host: `font_dir`, `hardware_decode`, `gpu`, a `GpuSettings` (backends, power preference, fallback adapter), and `temp_dir`, where a render keeps its output while encoding (unset, the system temporary directory). Its default has no font directory, decodes video on the CPU and picks the high-performance adapter on any backend. The `lumen-server` and `lumen-local` binaries build it from `LUMEN_FONT_DIR`, `LUMEN_HARDWARE_DECODE` and `GpuSettings::from_env()` (`WGPU_BACKEND`, `WGPU_POWER_PREF`, `LUMEN_GPU_FORCE_FALLBACK_ADAPTER`, ...) as before, so they behave the same; `lumen-server` also takes `--font-dir`. `lumen-gpu` and `lumen-engine` gain `GpuSettings` and `new_with_settings` constructors that read nothing; `Renderer::new()` and `GpuCompositionRenderer::new()` still read the environment through `GpuSettings::from_env()`.

`QualityConfig::resolve` in `lumen-ffmpeg` no longer reads `LUMEN_VIDEO_ENCODER`. A request pins the encoder it names, and a binary that wants the variable calls `QualityRequest::with_env_encoder()` first, which keeps the request's own encoder ahead of the variable and ignores an empty one. `lumen-server` and `lumen-local` do, so they behave the same; a render without `RenderOptions::encoder` uses the built-in presets. `LUMEN_MEDIA_ROOT` is gone: `RenderOptions::media_root` defaults to the current directory. `font_dir_from_env` is removed.

Before:

```rust
// LUMEN_FONT_DIR, LUMEN_HARDWARE_DECODE and LUMEN_MEDIA_ROOT were read from the process.
let options = RenderOptions { media_root: Some(dir), ..RenderOptions::default() };
let store = LocalMediaStore::new(root, font_dir.as_deref());
let diagnostics = check_composition(&payload, &staged, &assets);
match render_project_mp4(&composition, &options, &mut |_| {}) {
    Err(RenderError { code: "render_failed", message, retryable, .. }) => { /* any failure while rendering */ }
    ...
}
```

After:

```rust
use lumen_server::render::{ErrorCode, RenderConfig, RenderOptions, render_project};

let config = RenderConfig { font_dir: Some("/srv/fonts".into()), hardware_decode: true, ..RenderConfig::default() };
let options = RenderOptions { config: config.clone(), media_root: Some(dir), ..RenderOptions::default() };
let store = LocalMediaStore::new(root, &config);
let diagnostics = check_composition(&payload, &staged, &assets, &config);
match render_project(&composition, &options, &mut |_| {}) {
    Err(error) if error.code() == ErrorCode::Engine("media.not_found") => { /* error.engine_error() */ }
    Err(error) if error.code() == ErrorCode::EncodeFailed && error.retryable() => { /* retry */ }
    Err(error) => eprintln!("{}: {error}", error.code().as_str()),
    Ok(rendered) => { /* rendered.bytes */ }
}
```

`RenderError` is renamed `ServerRenderError`, so it no longer shares a name with `lumen_engine::RenderError`. Its fields are private: `code()` returns an `ErrorCode`, `retryable()` says whether to try again, `Display` is the message, and `source()` is the cause. An engine failure keeps the engine error as its source and has its diagnostic code, such as `media.not_found`, `media.decode` or `render.gpu`; of those `render.gpu` is retryable, and a media failure only when it is a shortage that can pass, such as running out of file descriptors. The library's own codes are `invalid_project_payload`, `invalid_quality`, `invalid_render_profile`, `invalid_config`, `render_failed`, `encode_failed`, `output_io`, `invalid_output`, `output_too_large` and `render_cancelled`. `audio_render_failed` is now the engine's code when mixing the audio failed and `encode_failed` when encoding it did, `invalid_fps` is `invalid_render_profile`, and `media_root_error` and `font_dir_error` are `invalid_config`. An encoder that fails to start keeps the FFmpeg error as its source: failing on the output is `output_io`, running out of memory, file descriptors or hardware encoder sessions is retryable, and an invalid option or unsupported encoder is not.

The HTTP server's failed renders carry these codes in `render.error.code`, `progress.errorCode` and the `render.failed` event, so a render failing on a missing or corrupt source reports `media.not_found` or `media.decode`, not `render_failed`, and is no longer marked retryable.
