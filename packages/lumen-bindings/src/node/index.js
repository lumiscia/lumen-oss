import * as previewBindings from "./lumen_wasm.js";

export * from "./lumen_wasm.js";

export class LumenBindings {
  target = "node";

  preview() {
    return Promise.resolve(previewBindings);
  }
}

export function createLumenBindings() {
  return new LumenBindings();
}
