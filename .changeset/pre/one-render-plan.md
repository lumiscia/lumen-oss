---
"@lumiscia/lumen-bindings": patch
---

`lumen` and `lumen-server` check a render request in one place, `lumen_engine::render::plan`, so asking for video settings on an image output fails with the same message from both. The code is unchanged (`invalid_output` on the server).

| Where           | Before                                                                                                                                                     | Now                                                                                                      |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `lumen render`  | `--encoder, --quality, --codec, --bit-depth, --alpha, --bitrate, --crf, --qp and --preset are only supported when output is a video (.mp4, .mov or .webm)` | `quality, codec, bit depth, alpha, rate control and encoder only apply to the mp4, mov and webm formats` |
| `POST /renders` | `quality, codec, bitDepth and alpha only apply to the mp4, mov and webm formats`                                                                           | the same as `lumen render`                                                                               |

`lumen render` now reports these flags after it has read the composition, so a composition that fails to load is reported first. For embedders, `lumen_engine::render` adds `RenderRequest`, `RenderPlan`, `plan` and `CheckReport` (which `check_composition` now returns), and `QualityRequest::with_encoder_fallback` in `lumen-engine-ffmpeg` is public.
