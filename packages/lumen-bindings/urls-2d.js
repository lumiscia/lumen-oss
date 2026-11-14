// The 2D-only build, without 3D nodes, for pages that don't need them. Its wasm needs its own worker.
export const workerUrl = new URL("./dist/browser/worker-2d.js", import.meta.url);
export const wasmUrl = new URL("./wasm/lumen_wasm_2d_bg.wasm", import.meta.url);
