---
"@lumiscia/lumen-bindings": minor
---

**Breaking for hosts that match on `render.gpu`.** The code now means a GPU fault only: the device, its queue or a driver failed, which no composition causes and trying again may fix. It used to cover everything the render raised that no other code fit, so a composition the author had to fix looked like an infrastructure alarm.

Two new codes take those cases:

| Code              | Means                                                                                                                                                                                                                            | Was `render.gpu` for                                                                                                                                                                                                                                                                                                                                                                   |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `render.limit`    | The composition needs more than the device or engine allows. The author fixes it: less text, smaller media.                                                                                                                      | A frame whose uniform or storage data doesn't fit the device's buffers (for example text with a very large glyph limit), and a CPU video frame larger than the device's texture limit.                                                                                                                                                                                                 |
| `render.internal` | The engine, or a host's resolver or render target, broke a contract the render relies on. A bug to report, not a composition to fix. Trying again helps only for fonts that kept loading, which stay loaded for the next render. | A poisoned text, atlas or font-cache lock, a stale or missing recorded frame passed to `upload_bound_frame` or `submit_render`, an image resolver returning a video frame, a malformed YUV frame, an external texture of the wrong size, format or usage, fonts that never stopped loading, recorder misuse, and a CUDA or VideoToolbox target asked of a renderer on another backend. |

A `wgsl_shader` whose source the device refuses (a syntax error, a binding it can't use) is now `render.node_evaluation` at that node, since the source is the composition's.

The WASM renderer's errors now carry the diagnostic code as `code` for every render failure, not only media ones.

A video frame that can't be imported on the GPU and then also fails to decode on the CPU now has the media code of the CPU failure (`media.decode`, `media.not_found`, ...) with the same message, instead of `render.gpu`.

`render.gpu` remains for creating the device and for submitting, validating and reading back work on it. `lumen-server` still treats exactly `render.gpu` (and transient media failures) as retryable, so `render.limit` and `render.internal` are not retried.

Migration: a host that retried or alerted on `render.gpu` keeps doing so for real GPU faults; one that showed `render.gpu` to authors as "your composition is wrong" should show `render.limit` that way and report `render.internal` as an engine bug. Hosts must already tolerate codes they don't know. The `lumen_gpu::Recorder` now returns a `FrameError` (`TooLarge` or `Misuse`) inside its `anyhow::Error` for these cases.
