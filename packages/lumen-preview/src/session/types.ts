import type { LumenComposition } from "@lumiscia/lumen-types";

import type { LumenAudioEngineOptions } from "../audio-engine.js";
import type { LumenLogLevel } from "../index.js";
import type { MediaSourceInput, VideoSourceRegistration } from "../media/index.js";
import type { LumenPreviewRange } from "../playback-range.js";
import type { LumenPreviewContext, LumenPreviewError } from "../preview.js";
import type { LumenPreviewStatsCallback, LumenPreviewTimeUpdateCallback } from "./stats.js";

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
  /**
   * Plays on from the start of the range after reaching its end, without a gap, audio included.
   * Without it, playback stops at the end and `onEnded` is called. Defaults to false.
   */
  loop?: boolean | undefined;
  /** How many frames past the playhead to decode ahead. Defaults to 8. */
  lookaheadCount?: number | undefined;
  /** Defaults to "off". */
  logLevel?: LumenLogLevel | undefined;
  media?: LumenPreviewMedia | undefined;
  /** Silences the audio without changing `volume`, so un-muting restores it. Defaults to false. */
  muted?: boolean | undefined;
  /**
   * Called once when playback reaches the end of the range without `loop`. Not called when the user
   * pauses or seeks there, nor on each pass of a loop.
   */
  onEnded?: (() => void) | null | undefined;
  /** Called with each failure the preview state's `error` also shows. */
  onError?: ((error: LumenPreviewError) => void) | null | undefined;
  onStats?: LumenPreviewStatsCallback | null | undefined;
  /**
   * Called at most once for each frame shown, with the frame and its start time. Costs nothing
   * while unset, and unlike the preview state it doesn't re-render anything by itself.
   */
  onTimeUpdate?: LumenPreviewTimeUpdateCallback | null | undefined;
  /**
   * Plays only these frames, `out` exclusive; null plays the whole composition. Seeks land inside
   * it. An `out` past the end is cut to it, with a console warning; an `in` past the end is
   * ignored, which plays the whole composition.
   */
  range?: LumenPreviewRange | null | undefined;
  /**
   * How fast playback runs, from 0.25 to 4, where 1 is real time. Audio follows, with its pitch.
   * Defaults to 1.
   */
  rate?: number | undefined;
  /** The audio level from 0 to 1. Defaults to 1. */
  volume?: number | undefined;
}

export interface LumenPreviewSessionOptions extends LumenPreviewSessionInputs {
  audio?: LumenAudioEngineOptions;
  preview: LumenPreviewContext;
  /** The wasm the worker loads: `wasmUrl` from `@lumiscia/lumen-bindings/urls`, or a copy of it. */
  wasmUrl: string | URL;
  /** The worker script: `workerUrl` from `@lumiscia/lumen-bindings/urls`, or a copy of it. */
  workerUrl: string | URL;
}
