import type { LumenPreviewSessionInputs, LumenWorkerBindings } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.svelte.js";

export interface LumenCanvasProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
  bindings: LumenWorkerBindings;
  /** Applied to the element that holds the canvas, which fills it and keeps its aspect ratio. */
  class?: string;
  style?: string;
}
