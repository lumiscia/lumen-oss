import type { LumenComposition, MediaSourceKind } from "@lumiscia/lumen-types";
import type { InputFormat } from "mediabunny";

import type { LumenAudioEngineOptions } from "../audio-engine.js";
import type { LumenLogLevel } from "../runtime.js";
import type { MediaSourceInput } from "../media/index.js";
import type { LumenPreviewRange } from "../playback-range.js";
import type { LumenPreviewContext, LumenPreviewError } from "../preview.js";
import type { LumenPreviewStatsCallback, LumenPreviewTimeUpdateCallback } from "./stats.js";

export type LumenPreviewComposition = LumenComposition | { toJSON(): LumenComposition };

/**
 * `model` is a binary glTF (`.glb`) file, which a `model_3d` reads, and `environment` a Radiance
 * `.hdr` or OpenEXR `.exr` file, which a `render_3d` lights with. `lut` is an Adobe/Resolve `.cube`
 * 3D table, which a `color_grade` reads through `lut_source`.
 */
export type LumenPreviewMediaKind = MediaSourceKind;

export interface LumenPreviewMediaEntry {
  /** The rate source frames are numbered at, for a video. Defaults to the stream's own rate. */
  fps?: number | null;
  /** The input formats to try for a video, instead of all of them. */
  formats?: InputFormat[];
  /**
   * Needed only for a source that only an expression names, when its blob's type doesn't say. A
   * kind the composition doesn't read the source as fails as `media.wrong_kind`.
   */
  kind?: LumenPreviewMediaKind;
  source: MediaSourceInput;
  /** Which track of a video to read. */
  track?: number | "primary";
}

/**
 * The media of each source the composition names, under the source as written: a name or path such
 * as `media_in.source`, `model_3d.source`, `render_3d.environment_map` or an audio clip's
 * `source_id` holds, or an `asset:<id>`. Entries the composition doesn't name aren't loaded,
 * unless an expression computes a source.
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
  /**
   * A directory holding the preview's runtime files under their own names: `worker.js` and
   * `lumen_wasm_bg.wasm` from `@lumiscia/lumen-bindings`, and `audio-worker.js` and
   * `audio-worklet.js` from `@lumiscia/lumen-preview`. Each URL not passed on its own is that
   * file in it.
   */
  runtimeUrl?: string | URL | undefined;
  /**
   * The wasm the worker loads: `wasmUrl` from `@lumiscia/lumen-bindings/urls`, or a copy of it.
   * Needed unless `runtimeUrl` is passed.
   */
  wasmUrl?: string | URL | undefined;
  /**
   * The worker script: `workerUrl` from `@lumiscia/lumen-bindings/urls`, or a copy of it. Needed
   * unless `runtimeUrl` is passed.
   */
  workerUrl?: string | URL | undefined;
}
