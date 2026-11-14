/// <reference types="svelte" />
import type { LumenPreviewSessionInputs, LumenRuntime } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.svelte.js";

export interface PlayerProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
  /**
   * The runtime files to load. Defaults to `runtime` from `@lumiscia/lumen-bindings/urls`, or
   * `runtime2d` from `@lumiscia/lumen-bindings/urls-2d` for the `Player` from
   * `@lumiscia/lumen-svelte/2d`.
   */
  runtime?: LumenRuntime | undefined;
  /**
   * A directory serving every file of `runtime` under its `file` name. Without it, each file is
   * loaded from its `url`.
   */
  runtimeUrl?: string | URL | undefined;
  /** Applied to the element that holds the canvas, which fills it and keeps its aspect ratio. */
  class?: string;
  style?: string;
}
