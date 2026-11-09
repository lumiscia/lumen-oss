---
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-bindings": minor
---

`POST /renders` on `lumen-server` accepts `quality`, a quality preset name such as `"high"`, and `codec`, `"h264"`, `"hevc"` or `"av1"`. Both win over the composition's `render_settings`. `RenderOptions.quality` and `RenderOptions.codec` in the SDK send them, and `QualityPreset` types the preset name.
