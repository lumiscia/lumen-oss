---
"@lumiscia/lumen-bindings": patch
---

Scale glyph outlines directly with unhinted `skrifa` and `zeno` instead of going through `cosmic_text::SwashCache` and `swash`, removing unused font-autohinting and duplicate scaler code from the browser WebAssembly binaries (-240 KB raw / -66 KB brotli-11 full; -129 KB raw / -18 KB brotli-11 2D) with no visual or API changes.
