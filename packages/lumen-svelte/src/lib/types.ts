/// <reference types="svelte" />
import type { LumenPreviewSessionInputs } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.svelte.js";

export interface PlayerProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
  /**
   * A directory serving the preview's runtime files: `worker.js` and `lumen_wasm_bg.wasm` (or
   * `worker-2d.js` and `lumen_wasm_2d_bg.wasm`, for the `Player` from `@lumiscia/lumen-svelte/2d`)
   * from `@lumiscia/lumen-bindings`, and `audio-worker.js` and `audio-worklet.js` from
   * `@lumiscia/lumen-preview`. When set, every runtime URL not passed on its own is that file in
   * it; otherwise each is found next to its package.
   */
  runtimeUrl?: string | URL | undefined;
  /** Where the preview worker's wasm is served. Defaults to the copy in `@lumiscia/lumen-bindings`. */
  wasmUrl?: string | URL | undefined;
  /** Where the preview worker is served. Defaults to the copy in `@lumiscia/lumen-bindings`. */
  workerUrl?: string | URL | undefined;
  /** Applied to the element that holds the canvas, which fills it and keeps its aspect ratio. */
  class?: string;
  style?: string;
}

/** The wasm and worker a `Player` falls back to, and their file names in `runtimeUrl`. */
export interface PlayerDefaults {
  wasmUrl: URL;
  wasmFile: string;
  workerUrl: URL;
  workerFile: string;
}
