import { createLumenPreviewWorkerHost } from "@lumiscia/lumen-preview/worker-host";

import initPreview, * as previewBindings from "../../wasm/lumen_wasm_2d.js";

createLumenPreviewWorkerHost({ initPreview, previewBindings });
