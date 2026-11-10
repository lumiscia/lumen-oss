---
"@lumiscia/lumen-bindings": patch
---

Load fonts faster: each font file is parsed once instead of on every later font load, and the families text nodes name as literals load before a composition's first frame is recorded, so that frame is not recorded again for them.
