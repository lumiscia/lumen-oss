import type { LumenComposition } from "@lumiscia/lumen-types";

import type { LumenAudioEngineOptions } from "../audio-engine.js";
import type { LumenLogLevel } from "../index.js";
import type { MediaSourceInput, VideoSourceRegistration } from "../media/index.js";
import type { LumenPreviewContext, LumenPreviewError } from "../preview.js";
import type { LumenPreviewStatsCallback } from "./stats.js";

export type LumenPreviewComposition = LumenComposition | { toJSON(): LumenComposition };

/** `model` is a binary glTF (`.glb`) file, which a `model_3d` reads. */
export type LumenPreviewMediaKind = "audio" | "image" | "model" | "video";

export interface LumenPreviewMediaEntry extends Omit<VideoSourceRegistration, "source"> {
  /** Needed only when the blob's type or the composition doesn't already say. */
  kind?: LumenPreviewMediaKind;
  source: MediaSourceInput;
}

/**
 * What each alias in the composition reads, as `media_in.source`, `model_3d.source` and audio
 * `source_id` name it.
 */
export type LumenPreviewMedia = Readonly<Record<string, MediaSourceInput | LumenPreviewMediaEntry>>;

/** Bytes or a URL for each font family the composition names. */
export type LumenPreviewFonts = Readonly<Record<string, MediaSourceInput>>;

export interface LumenPreviewSessionInputs {
  composition?: LumenPreviewComposition | null | undefined;
  fonts?: LumenPreviewFonts | undefined;
  /** How many frames past the playhead to decode ahead. Defaults to 8. */
  lookaheadCount?: number | undefined;
  /** Defaults to "off". */
  logLevel?: LumenLogLevel | undefined;
  media?: LumenPreviewMedia | undefined;
  /** Called with each failure the preview state's `error` also shows. */
  onError?: ((error: LumenPreviewError) => void) | null | undefined;
  onStats?: LumenPreviewStatsCallback | null | undefined;
}

export interface LumenPreviewSessionOptions extends LumenPreviewSessionInputs {
  audio?: LumenAudioEngineOptions;
  preview: LumenPreviewContext;
  /** The wasm the worker loads: `wasmUrl` from `@lumiscia/lumen-bindings/urls`, or a copy of it. */
  wasmUrl: string | URL;
  /** The worker script: `workerUrl` from `@lumiscia/lumen-bindings/urls`, or a copy of it. */
  workerUrl: string | URL;
}
