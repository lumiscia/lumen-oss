import * as previewBindings from "./lumen_wasm.js";

export { LumenMediaStore, LumenPreviewController, LumenRenderer } from "./lumen_wasm.js";

export class LumenBindings {
  target = "node";

  preview() {
    return Promise.resolve(previewBindings);
  }
}

export function createLumenBindings() {
  return new LumenBindings();
}
