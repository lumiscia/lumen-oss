---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
---

The frame graph is now the only renderer. The compiled plan path and the `LUMEN_FRAME_GRAPH` flag are gone, so every frame is recorded with pooled textures and cached programs.

Behaviour changes:

- A shader that writes fewer pixels than it did on the previous frame no longer leaves the old pixels behind. What it doesn't write is transparent.
- If a source's first output frame shows it twice and only the second import fails, the render fails with the "would change" error instead of moving both uses to CPU decoding.
- Video jobs can hold more VRAM.
- Shaders chosen by an expression build at first use. A bad literal shader still fails at compile time when the output can reach it.
- `compile()` now checks every branch connected to the output, not only what frame 0 shows, even if no frame ever reaches it. A bad literal shader or uniform under a clip that hasn't started, a zero-duration clip, or any layer of an expression switch, including one like `selected_layer: "=0"`, fails at compile. Disconnected nodes, unplayed templates and the unselected layers of a literal switch still aren't checked.
- `compile()` no longer evaluates uniform expressions, so one giving the wrong component count fails at the frame that uses it. Literal uniforms are still checked at compile, including the length of a vector with expression components.
- `render_frame_into_external` accepts more targets: a `COPY_DST` texture of the right size and format no longer needs the other usages. A wrong size or format is still refused, with a reworded error.

For the preview: the WASM `LumenPreviewController` no longer has `setLookaheadCount`, because there is nothing to build ahead. `setLookaheadCount` on the `@lumiscia/lumen-preview` controller and the `lookaheadCount` prop still set how far ahead media loads.

For Rust: `lumen-engine-gpu` loses `RenderPlan`, `RenderPlanBuilder` and the plan resources, keeping `Recorder`. `GpuCompositionRenderer` loses `compile_with_media`, `precompile_frame*`, `set_plan_cache_budget`, `compiled` and the plan counters. `plan_cache_bytes` is now `held_bytes` and `cached_pass_count` is now `skipped_pass_count`. `RasterHandle` holds the texture itself, and `ExternalTexture` constructors no longer take a domain. `lumen-local`'s summary prints `held_bytes`, `skipped_passes` and `submitted_passes`.
