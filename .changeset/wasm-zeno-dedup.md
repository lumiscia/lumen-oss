---
"@lumiscia/lumen-bindings": patch
---

Share one `zeno` glyph rasterizer instantiation between `lumen-text` and `swash`, reducing the browser WebAssembly binaries by 133 KB raw (~11 KB brotli-11 full, ~12 KB brotli-11 2D) with no visual or API changes.
