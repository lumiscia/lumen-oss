// Plain JavaScript beside the build output, so packages can import it before lumen-bindings is built.
// Bundlers such as Vite emit each file as an asset, and without one they resolve against this file.
export const runtime = {
  worker: { file: "worker.js", url: new URL("./dist/browser/worker.js", import.meta.url) },
  wasm: { file: "lumen_wasm_bg.wasm", url: new URL("./wasm/lumen_wasm_bg.wasm", import.meta.url) },
  audioWorker: {
    file: "audio-worker.js",
    url: new URL("./dist/browser/audio-worker.js", import.meta.url),
  },
  audioWorklet: {
    file: "audio-worklet.js",
    url: new URL("./dist/browser/audio-worklet.js", import.meta.url),
  },
};
