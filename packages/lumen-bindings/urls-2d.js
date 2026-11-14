// The 2D-only build, without 3D nodes, for pages that don't need them. Its wasm needs its own worker.
export const runtime2d = {
  worker: { file: "worker-2d.js", url: new URL("./dist/browser/worker-2d.js", import.meta.url) },
  wasm: {
    file: "lumen_wasm_2d_bg.wasm",
    url: new URL("./wasm/lumen_wasm_2d_bg.wasm", import.meta.url),
  },
  audioWorker: {
    file: "audio-worker.js",
    url: new URL("./dist/browser/audio-worker.js", import.meta.url),
  },
  audioWorklet: {
    file: "audio-worklet.js",
    url: new URL("./dist/browser/audio-worklet.js", import.meta.url),
  },
};
