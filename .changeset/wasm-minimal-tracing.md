---
"@lumiscia/lumen-bindings": patch
---

Replace `tracing-subscriber` and `wasm-tracing` in the browser WebAssembly crate with a minimal console subscriber, reducing both browser WebAssembly binaries by 34 KB raw (~10–11 KB brotli-11) while keeping all runtime `setLogLevel` levels (`off`, `error`, `warn`, `info`, `debug`, `trace`). Browser console output now prints plain `[LEVEL target] message key=value` lines without `wasm-tracing`'s collapsible span groups.
