/// <reference types="svelte" />
import type { LumenPreviewSessionInputs } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.svelte.js";

export interface PlayerProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
  /** Where the preview worker's wasm is served. Defaults to the copy in `@lumiscia/lumen-bindings`. */
  wasmUrl?: string | URL | undefined;
  /** Where the preview worker is served. Defaults to the copy in `@lumiscia/lumen-bindings`. */
  workerUrl?: string | URL | undefined;
  /** Applied to the element that holds the canvas, which fills it and keeps its aspect ratio. */
  class?: string;
  style?: string;
}
