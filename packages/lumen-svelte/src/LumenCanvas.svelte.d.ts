import type { Component } from "svelte";
import type { LumenPreviewBindings } from "lumen-preview";
import type { LumenPreviewContext } from "./preview.svelte.js";

export type LumenCanvasProps = {
  preview: LumenPreviewContext;
  bindings: LumenPreviewBindings;
  compositionJson?: string | null;
  fps?: number;
  class?: string;
  style?: string;
};

declare const LumenCanvas: Component<LumenCanvasProps>;
export default LumenCanvas;
