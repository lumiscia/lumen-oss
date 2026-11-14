import type { LumenAudioEngine } from "../audio-engine.js";
import type { PreviewClock } from "../clock.js";
import type { MediaSourceInput } from "../media/index.js";
import type { PreviewMediaEntry } from "../media/dependencies.js";
import type { MediaEntryState } from "../media/lifecycle.js";
import { DEFAULT_PLAYBACK, type PlaybackRange, type PlaybackSettings } from "../playback-range.js";
import type { LumenPreviewWorkerCommand } from "../worker-host.js";
import type { ResolvedPreviewInputs } from "./inputs.js";

export const PARKED_CLOCK: PreviewClock = { atEpochMs: 0, originMs: 0, rate: 0 };

export type PreviewWorkerOptions = Omit<
  Extract<LumenPreviewWorkerCommand, { type: "set-options" }>,
  "type"
>;

/**
 * Everything one attach to a host element owns: the worker, the canvas and the audio, and the
 * playback and load state that goes with them. Detaching disposes it and drops it, so nothing
 * of it survives into the next attach.
 */
export class Attachment {
  readonly startup = new AbortController();
  audio: LumenAudioEngine | null = null;
  canvas: HTMLCanvasElement | null = null;
  worker: Worker | null = null;
  clock = PARKED_CLOCK;
  correctionTimer: ReturnType<typeof setInterval> | null = null;
  // Whether the worker is still loading the media of the frame playback is on.
  frameWaiting = false;
  // Whether playing is waiting, with its clock parked, for media to load.
  held = false;
  // The newest load the worker has loaded and drawn a frame of, and what it last said of that
  // load's media.
  loadedId = 0;
  shownId = 0;
  media: readonly MediaEntryState[] | null = null;
  // Whether a load is in flight. A seek waits for it, to be clamped to the new length.
  loading = false;
  // What was last asked for, whether or not it worked: only a change is worth trying again.
  loaded: ResolvedPreviewInputs | null = null;
  // A seek made without a loaded timeline, applied once one loads.
  pendingSeek: number | null = null;
  pendingSeeks = new Map<number, (presented: boolean) => void>();
  // Counts the changes that make a queued `ended` stale: loop, range, rate, pause and play. The
  // worker follows it through the clocks it is sent, and its `ended` events name the one they ended under.
  playbackRevision = 0;
  // The last options that were valid, which are the ones in effect.
  playback: PlaybackSettings = DEFAULT_PLAYBACK;
  playing = false;
  // The part of the loaded timeline being played.
  range: PlaybackRange | null = null;
  // What was last said about a range that didn't fit. The same message isn't reported again, so a
  // callback that re-renders can't loop.
  rangeMisfit: string | null = null;
  // Why the inputs last failed to resolve. The same failure isn't reported again, so a callback that
  // re-renders with the same inputs can't loop.
  resolveFailure: string | null = null;
  // The frame onTimeUpdate last reported, so a frame already shown isn't reported again.
  reportedFrame: number | null = null;
  // The next worker counts its seeks from zero.
  seekId = 0;
  sentFonts: ReadonlyMap<string, MediaSourceInput> = new Map();
  sentMedia: ReadonlyMap<string, PreviewMediaEntry> = new Map();
  timeline: { fps: number; totalFrames: number } | null = null;

  constructor(public workerOptions: PreviewWorkerOptions) {}

  /** Playing, and not held for media. */
  get running(): boolean {
    return this.playing && !this.held;
  }

  post(message: LumenPreviewWorkerCommand, transfer: Transferable[] = []): void {
    this.worker?.postMessage(message, transfer);
  }

  stopCorrections(): void {
    if (this.correctionTimer !== null) {
      clearInterval(this.correctionTimer);
      this.correctionTimer = null;
    }
  }

  settleSeeks(presented: boolean): void {
    for (const resolve of this.pendingSeeks.values()) {
      resolve(presented);
    }
    this.pendingSeeks.clear();
  }

  dispose(): void {
    this.startup.abort();
    this.stopCorrections();
    this.settleSeeks(false);
    this.audio?.dispose();
    this.worker?.terminate();
    this.canvas?.remove();
  }
}
