---
"@lumiscia/lumen-bindings": patch
---

The browser build is smaller: it links one copy of the font-reading crates instead of two, which takes about 24 KB off each brotli-compressed file (1,900,844 to 1,877,089 bytes for the full build, 1,430,044 to 1,406,406 for the 2D build).
