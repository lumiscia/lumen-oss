---
"@lumiscia/lumen-bindings": minor
---

**Breaking (Rust, `lumen-server` library):** `lumen_server::render` reads no environment variables, and its error has a code to match on.

A `RenderConfig` holds what renders take from the host: `font_dir` and `hardware_decode`. Its default has no font directory and decodes video on the CPU. The `lumen-server` and `lumen-local` binaries build it from `LUMEN_FONT_DIR` and `LUMEN_HARDWARE_DECODE` as before, so they behave the same; `lumen-server` also takes `--font-dir`. `LUMEN_MEDIA_ROOT` is gone: `RenderOptions::media_root` defaults to the current directory. `font_dir_from_env` is removed.

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
use lumen_server::render::{ErrorCode, RenderConfig, RenderOptions, ServerRenderError};

let config = RenderConfig { font_dir: Some("/srv/fonts".into()), hardware_decode: true };
let options = RenderOptions { config: config.clone(), media_root: Some(dir), ..RenderOptions::default() };
let store = LocalMediaStore::new(root, &config);
let diagnostics = check_composition(&payload, &staged, &assets, &config);
match render_project_mp4(&composition, &options, &mut |_| {}) {
    Err(error) if error.code() == ErrorCode::Engine("media.not_found") => { /* error.engine_error() */ }
    Err(error) if error.code() == ErrorCode::EncodeFailed && error.retryable() => { /* retry */ }
    Err(error) => eprintln!("{}: {error}", error.code().as_str()),
    Ok(mp4) => { /* ... */ }
}
```

`RenderError` is renamed `ServerRenderError`, so it no longer shares a name with `lumen_engine::RenderError`. Its fields are private: `code()` returns an `ErrorCode`, `retryable()` says whether to try again, `Display` is the message, and `source()` is the cause. An engine failure keeps the engine error as its source and has its diagnostic code, such as `media.not_found`, `media.decode` or `render.gpu`; of those only `render.gpu` is retryable. The library's own codes are `invalid_project_payload`, `invalid_quality`, `invalid_render_profile`, `invalid_config`, `render_failed`, `encode_failed` and `output_io`. `audio_render_failed`, `invalid_fps`, `media_root_error` and `font_dir_error` are gone, into the engine's codes, `encode_failed`, `invalid_render_profile` and `invalid_config`.

The HTTP server's failed renders carry these codes in `render.error.code`, `progress.errorCode` and the `render.failed` event, so a render failing on a missing or corrupt source reports `media.not_found` or `media.decode`, not `render_failed`, and is no longer marked retryable.
