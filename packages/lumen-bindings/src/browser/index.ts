import type { LumenBindings } from "@lumiscia/lumen-preview";

import { createBindings } from "../bindings.js";

export { initSync, LumenPreviewController } from "../../wasm/lumen_wasm.js";
export type { InitInput, InitOutput, SyncInitInput } from "../../wasm/lumen_wasm.js";

/** For pages that host the wasm themselves. */
export interface LumenBrowserBindingsOptions {
  previewWasmUrl?: string | URL;
}

/** Loads the wasm on this thread, to parse compositions and answer timeline questions. */
export function createLumenBindings(options: LumenBrowserBindingsOptions = {}): LumenBindings {
  return createBindings(
    options.previewWasmUrl ?? new URL("../../wasm/lumen_wasm_bg.wasm", import.meta.url),
  );
}
