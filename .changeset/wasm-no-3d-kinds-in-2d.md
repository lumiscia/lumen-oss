---
"@lumiscia/lumen-bindings": patch
---

Gate the 13 3D `NodeKind` variants and their `ParamType` / JSON evaluation / scope-check code behind `#[cfg(feature = "3d")]`, reducing the 2D browser WebAssembly binary (`lumen_wasm_2d_bg.wasm`) by 244.8 KB raw (~28.8 KB brotli-11) while keeping `node.unsupported` diagnostics unchanged when a 3D node appears in a 2D composition.
