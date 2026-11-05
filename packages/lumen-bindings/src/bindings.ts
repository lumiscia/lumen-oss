import type { LumenWorkerBindings } from "@lumiscia/lumen-preview";

import initPreview, * as previewBindings from "../wasm/lumen_wasm.js";

export function createBindings(
  wasmUrl: string | URL,
  workerUrl: string | URL,
): LumenWorkerBindings {
  return {
    preview: async () => {
      await initPreview({ module_or_path: wasmUrl });
      return previewBindings;
    },
    previewWasmUrl: () => wasmUrl,
    previewWorkerUrl: () => workerUrl,
  };
}
