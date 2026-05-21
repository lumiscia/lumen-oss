import previewWorkerModuleUrl from "./preview-worker.js?worker&url";
import * as previewBindings from "./lumen_wasm.js";

export { LumenMediaStore, LumenPreviewController, LumenRenderer } from "./lumen_wasm.js";

export class LumenBindings {
  target = "bundler";

  preview() {
    return Promise.resolve(previewBindings);
  }

  previewWorkerUrl() {
    return previewWorkerModuleUrl;
  }
}

export function createLumenBindings() {
  return new LumenBindings();
}
