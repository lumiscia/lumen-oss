import initPreview, * as previewBindings from "./lumen_wasm.js";

export * from "./lumen_wasm.js";

export class LumenBindings {
  target = "browser";

  constructor(options = {}) {
    this.previewWasmUrl =
      options.previewWasmUrl ?? new URL("./lumen_wasm_bg.wasm", import.meta.url);
    this.previewWorkerModuleUrl = options.previewWorkerUrl;
  }

  async preview() {
    await initPreview({ module_or_path: this.previewWasmUrl });
    return previewBindings;
  }

  previewWorkerUrl() {
    return this.previewWorkerModuleUrl;
  }
}

export function createLumenBindings(options = {}) {
  return new LumenBindings(options);
}
