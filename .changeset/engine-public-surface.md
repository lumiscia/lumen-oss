---
"@lumiscia/lumen-bindings": minor
---

`lumen-engine`'s renderer has one render call, and the crate's public surface is narrower.

`GpuCompositionRenderer::render(composition, frame, media, target)` renders and submits a frame. `frame` is any `Into<f64>` in composition frames, so a `u32` frame and a fractional time take the same call. `target` is `RenderTarget::Owned` (a texture of the renderer's) or `RenderTarget::External(ExternalTexture)`. It returns a `RenderedFrame` with the output `raster` (the external texture for an external target) and its `submission`, and `wait(&device)` blocks until the GPU has finished it. Hosts that time the stages apart call `bind_frame`, `upload_frame` and `submit_frame`, which `render` runs in turn. Rendered output, performance and the server's behaviour are unchanged.

**Breaking, for Rust hosts:**

| Removed                                                        | Use instead                                                                                                                                            |
| -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `render_frame(c, frame, m)`, `render_frame_at(c, time, m)`     | `render(c, frame, m, RenderTarget::Owned)?.raster`                                                                                                     |
| `render_frame_submitted(c, frame, m)` → `(raster, submission)` | `render(c, frame, m, RenderTarget::Owned)?` → `RenderedFrame { raster, submission }`                                                                   |
| `render_frame_into_external(c, frame, m, texture)`             | `render(c, frame, m, RenderTarget::External(texture))?`, a `RenderedFrame` in place of `SubmittedExternalTexture` (`.raster.texture`, `.wait(device)`) |
| `bind_frame(c, frame, m)`, `bind_frame_at(c, time, m)`         | `bind_frame(c, frame, m, target)`                                                                                                                      |
| `upload_bound_frame(&bound)`                                   | `upload_frame(&bound)`                                                                                                                                 |
| `submit_render()`                                              | `submit_frame(&bound)`, which refuses a frame recorded before the latest, as uploading does                                                            |
| `submit_bound_frame(&bound)`                                   | `upload_frame(&bound)` then `submit_frame(&bound)`                                                                                                     |

- The test-only renderer hooks (`limit_frame_storage`, `recorded_domain`, `recorded_pass_count`, `text_atlas_texture`, `set_full_frame_sources`, `set_single_tile_repeats`, `cached_media_texture_count`, `media_texture_upload_count`) and `BoundFrame`'s `atlas_region_count`, `atlas_region_rects` and `glyph_instances` exist only with the new `test-hooks` cargo feature, which is not a supported host API.
- `lumen_engine::scope` is no longer public and `lumen_engine::fingerprint` is hidden from the docs (the derive macros name it); `gpu::MediaTextureKey`, `Clip::window`, `ClipWindow::local_time` and `node::source::media_in::map_to_source_frame` are no longer public either.
- The `lumen_engine::ffmpeg` and `lumen_engine::image` aliases and the `media::ffmpeg` and `media::image` modules are gone: `FfmpegVideoResolver`, `FfmpegAudioResolver`, `FfmpegResolverOptions` and `ImageFileResolver` are at `lumen_engine::media::*`, where they already were too.

Compositions, the TypeScript SDK and the server's HTTP API are unaffected.
