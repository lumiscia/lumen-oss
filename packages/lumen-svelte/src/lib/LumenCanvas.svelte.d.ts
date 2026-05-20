import type { Component } from "svelte";
import type {
  AudioSourceRegistration,
  LumenLogLevel,
  LumenPreviewBindingSource,
  LumenPreviewStatsCallback,
  MediaRegistration,
} from "@lumiscia/lumen-preview";
import type { LumenPreviewContext } from "./preview.svelte.js";

export type LumenCanvasProps = {
  preview: LumenPreviewContext;
  bindings: LumenPreviewBindingSource;
  audioSources?: AudioSourceRegistration[];
  compositionJson?: string | null;
  mediaSources?: MediaRegistration[];
  logLevel?: LumenLogLevel;
  onStats?: LumenPreviewStatsCallback;
  class?: string;
  style?: string;
};

declare const LumenCanvas: Component<LumenCanvasProps>;
export default LumenCanvas;
