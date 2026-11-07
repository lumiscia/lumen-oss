---
"@lumiscia/lumen-bindings": patch
---

A `path` or `shape` stroke under a pixel wide now covers at most its width of a pixel, so a stroke animated to zero width fades out instead of popping off on its last frame. Centred on a pixel, a 0.1 px stroke drew at about 55% alpha and now draws at 10%. Strokes 1 px wide and up are unchanged.
