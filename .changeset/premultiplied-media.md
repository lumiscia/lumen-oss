---
"@lumiscia/lumen-bindings": minor
---

Translucent images and video now render correctly in `lumen-local` and `lumen-server`: their pixels are premultiplied when decoded, as the browser preview already did, so PNGs, ProRes 4444 and other alpha video stop rendering too bright and stop leaking colour from transparent pixels. VP8 and VP9 WebM alpha is detected and decoded with libvpx, and renders opaque with a warning on FFmpeg builds without it. `wgsl_shader` inputs from these sources now hold premultiplied colour, as documented.
