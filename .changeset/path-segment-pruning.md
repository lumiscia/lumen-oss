---
"@lumiscia/lumen-bindings": patch
---

Paths with a large fill offset or feather render several times faster: each pixel now measures only the segments that can decide its coverage, and skips that work entirely where the fill is known to cover all or none of it. Rendered output is unchanged.
