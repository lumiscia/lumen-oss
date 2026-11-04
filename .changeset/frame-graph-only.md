---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
---

The frame graph is now the only renderer. The compiled plan path and the `LUMEN_FRAME_GRAPH` flag are gone, so every frame is recorded with pooled textures and cached programs.

Behaviour changes:

- A shader that writes fewer pixels than it did on the previous frame no longer leaves the old pixels behind. What it doesn't write is transparent.
- If a source's first output frame shows it twice and only the second import fails, the render fails with the "would change" error instead of moving both uses to CPU decoding.
- Video jobs can hold more VRAM.
- Shaders chosen by an expression build at first use. A bad literal shader still fails at compile time, before any frame renders.

For the preview: the WASM `LumenPreviewController` no longer has `setLookaheadCount`, because there is nothing to build ahead. `setLookaheadCount` on the `@lumiscia/lumen-preview` controller and the `lookaheadCount` prop still set how far ahead media loads.

For Rust: `lumen-engine-gpu` loses `RenderPlan`, `RenderPlanBuilder` and the plan resources, keeping `Recorder`. `GpuCompositionRenderer` loses `compile_with_media`, `precompile_frame*`, `set_plan_cache_budget`, `compiled` and the plan counters. `plan_cache_bytes` is now `held_bytes` and `cached_pass_count` is now `skipped_pass_count`. `RasterHandle` holds the texture itself, and `ExternalTexture` constructors no longer take a domain. `lumen-local`'s summary prints `held_bytes`, `skipped_passes` and `submitted_passes`.
