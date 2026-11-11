import { readFileSync } from "node:fs";

import type { LumenBindings } from "@lumiscia/lumen-preview";

import * as previewBindings from "../../wasm/lumen_wasm.js";

previewBindings.initSync({
  module: readFileSync(new URL("../../wasm/lumen_wasm_bg.wasm", import.meta.url)),
});

export { LumenPreviewController } from "../../wasm/lumen_wasm.js";

/** Parses compositions and answers timeline questions. Rendering needs a browser's WebGPU. */
export function createLumenBindings(): LumenBindings {
  return { preview: async () => previewBindings };
}
