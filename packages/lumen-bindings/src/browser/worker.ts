import { createLumenPreviewWorkerHost } from "@lumiscia/lumen-preview/worker-host";

import initPreview, * as previewBindings from "../../wasm/lumen_wasm.js";

createLumenPreviewWorkerHost({ initPreview, previewBindings });
