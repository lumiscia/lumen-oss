// Plain JavaScript beside the build output, so packages can import it before lumen-bindings is built.
// Bundlers such as Vite emit both files as assets, and without one they resolve against this file.
export const workerUrl = new URL("./dist/browser/worker.js", import.meta.url);
export const wasmUrl = new URL("./wasm/lumen_wasm_bg.wasm", import.meta.url);
