import type { LumenComposition } from "@lumiscia/lumen-types";

import type { LumenAudioEngineOptions } from "../audio-engine.js";
import type { LumenLogLevel, LumenWorkerBindings } from "../index.js";
import type { MediaSourceInput, VideoSourceRegistration } from "../media/index.js";
import type { LumenPreviewContext } from "../preview.js";
import type { LumenPreviewStatsCallback } from "./stats.js";

export type LumenPreviewComposition = LumenComposition | { toJSON(): LumenComposition };

export type LumenPreviewMediaKind = "audio" | "image" | "video";

export interface LumenPreviewMediaEntry extends Omit<VideoSourceRegistration, "source"> {
  /** Needed only when the blob's type or the composition doesn't already say. */
  kind?: LumenPreviewMediaKind;
  source: MediaSourceInput;
}

/** What each alias in the composition reads, as `media_in.source` and audio `source_id` name it. */
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
  onStats?: LumenPreviewStatsCallback | null | undefined;
}

export interface LumenPreviewSessionOptions extends LumenPreviewSessionInputs {
  audio?: LumenAudioEngineOptions;
  bindings: LumenWorkerBindings;
  preview: LumenPreviewContext;
}
