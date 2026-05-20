import { createLumenPreviewWorkerHost } from "@lumiscia/lumen-preview/worker-host";

import initPreview, * as previewBindings from "../browser/lumen_wasm.js";
import previewWasmUrl from "../browser/lumen_wasm_bg.wasm?url";

createLumenPreviewWorkerHost({ initPreview, previewBindings, previewWasmUrl });
