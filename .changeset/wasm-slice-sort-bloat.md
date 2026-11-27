---
"@lumiscia/lumen-bindings": patch
---

Eliminate redundant `core::slice::sort` and `BTreeMap::from_iter` instantiations across `lumen-engine`, `lumen-text`, and `lumen-macros`, reducing the browser WebAssembly binaries by 191 KB raw (~21 KB brotli-11 full, ~19.5 KB brotli-11 2D) with no behavioral changes.
