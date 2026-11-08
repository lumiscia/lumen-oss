---
"@lumiscia/lumen-bindings": patch
---

`POST /renders` on `lumen-server` resolves `quality` and `codec` when the render is requested, with the resolver the render uses, and returns `422 invalid_quality` for a preset the host doesn't define, a codec none of the encoders it tries has settings for, or a codec other than the one `LUMEN_VIDEO_ENCODER`'s encoder writes, instead of accepting the render and failing it later.
