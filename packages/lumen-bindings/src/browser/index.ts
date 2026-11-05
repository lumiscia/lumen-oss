import type { LumenWorkerBindings } from "@lumiscia/lumen-preview";

import { createBindings } from "../bindings.js";

export { initSync, LumenPreviewController } from "../../wasm/lumen_wasm.js";
export type { InitInput, InitOutput, SyncInitInput } from "../../wasm/lumen_wasm.js";

/** Overrides for pages that host the wasm or the worker themselves. */
export interface LumenBrowserBindingsOptions {
  previewWasmUrl?: string | URL;
  previewWorkerUrl?: string | URL;
}

export function createLumenBindings(
  options: LumenBrowserBindingsOptions = {},
): LumenWorkerBindings {
  return createBindings(
    options.previewWasmUrl ?? new URL("../../wasm/lumen_wasm_bg.wasm", import.meta.url),
    options.previewWorkerUrl ?? new URL("./worker.js", import.meta.url),
  );
}
