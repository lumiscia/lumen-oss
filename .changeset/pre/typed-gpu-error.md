---
"@lumiscia/lumen-bindings": minor
---

The `lumen_gpu::Recorder` methods a frame calls (`resolve`, `compute_program`, `dynamic_compute_program`, `render_program`, `upload` and `submit`) return `Result<_, FrameError>` instead of `anyhow::Result`, and `FrameError` has a `Device(String)` variant for what the device refuses, such as a failed validation. Match on the variant where you downcast `anyhow::Error` to `FrameError` before; `FrameError` still implements `std::error::Error`, so `?` into `anyhow` keeps working. `lumen_gpu::Renderer` is unchanged.
