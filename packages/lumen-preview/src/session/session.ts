import type { LumenDiagnostic, MediaFailure } from "@lumiscia/lumen-types";

import { LumenAudioEngine } from "../audio-engine.js";
import { clockPositionMs, epochNow, fitClock, type PreviewClock } from "../clock.js";
import {
  DEFAULT_PLAYBACK,
  describeRangeMisfit,
  rangeFrame,
  rangeMs,
  readPlaybackSettings,
  resolvePlaybackRange,
  type LumenPreviewRange,
  type PlaybackRange,
  type PlaybackSettings,
} from "../playback-range.js";
import type { PreviewMediaEntry } from "../media/dependencies.js";
import type { MediaSourceInput } from "../media/index.js";
import type { MediaEntryState } from "../media/lifecycle.js";
import { sameMediaInput } from "../media/source.js";
import { assetHint } from "../asset-hint.js";
import { createModuleWorker } from "../module-worker.js";
import type {
  LumenPreviewContext,
  LumenPreviewError,
  LumenPreviewErrorCode,
  LumenPreviewMediaState,
} from "../preview.js";
import type { LumenPreviewWorkerCommand, LumenPreviewWorkerEvent } from "../worker-host.js";
import { describeError, errorMediaFailure, reportConsoleError } from "./errors.js";
import {
  absoluteUrl,
  resolvePreviewInputs,
  sameMediaEntry,
  type ResolvedPreviewInputs,
} from "./inputs.js";
import { EMPTY_PREVIEW_STATS } from "./stats.js";
import type { LumenPreviewSessionInputs, LumenPreviewSessionOptions } from "./types.js";

const CLOCK_CORRECTION_INTERVAL_MS = 250;
const DEFAULT_LOOKAHEAD_COUNT = 8;
const NO_MEDIA: LumenPreviewMediaState = { failed: 0, pending: 0, ready: 0 };
const PARKED_CLOCK: PreviewClock = { atEpochMs: 0, originMs: 0, rate: 0 };
const WORKER_STARTUP_TIMEOUT_MS = 30_000;

type WorkerEvent<TType extends LumenPreviewWorkerEvent["type"]> = Extract<
  LumenPreviewWorkerEvent,
  { type: TType }
>;

type PreviewWorkerOptions = Omit<
  Extract<LumenPreviewWorkerCommand, { type: "set-options" }>,
  "type"
>;

class PreviewStartupError extends Error {
  constructor(
    readonly code: LumenPreviewErrorCode,
    message: string,
  ) {
    super(message);
  }
}

/**
 * Shows a composition in a canvas it creates inside a host element.
 *
 * The render loop lives in a worker that owns the canvas, and this side owns playback: it holds the
 * audio and tells the worker where the clock is. Every failure ends up in the preview's state.
 *
 * The worker loads the media the composition needs, and swaps to it once everything but its audio
 * is in. Playing holds its clock, at its position, while anything the composition needs is pending.
 */
export class LumenPreviewSession {
  readonly preview: LumenPreviewContext;
  // Bumped by every attach and dispose, so a slow startup can tell it has been replaced.
  #attachment = 0;
  #audio: LumenAudioEngine | null = null;
  readonly #audioOptions: LumenPreviewSessionOptions["audio"];
  #canvas: HTMLCanvasElement | null = null;
  #clock = PARKED_CLOCK;
  #correctionTimer: ReturnType<typeof setInterval> | null = null;
  // Whether the worker is still loading the media of the frame playback is on.
  #frameWaiting = false;
  // Whether playing is waiting, with its clock parked, for media to load.
  #held = false;
  #inputs: LumenPreviewSessionInputs;
  #loadId = 0;
  // The newest load the worker has loaded and drawn a frame of, and what it last said of that
  // load's media.
  #loadedId = 0;
  #shownId = 0;
  #media: readonly MediaEntryState[] | null = null;
  // Whether a load is in flight. A seek waits for it, to be clamped to the new length.
  #loading = false;
  // What was last asked for, whether or not it worked: only a change is worth trying again.
  #loaded: ResolvedPreviewInputs | null = null;
  // A seek made without a loaded timeline, applied once one loads.
  #pendingSeek: number | null = null;
  #pendingSeeks = new Map<number, (presented: boolean) => void>();
  // Counts the changes that make a queued `ended` stale: loop, range, rate, pause and play. The
  // worker follows it through the clocks it is sent, and its `ended` events name the one they ended under.
  #playbackRevision = 0;
  // The last options that were valid, which are the ones in effect.
  #playback: PlaybackSettings = DEFAULT_PLAYBACK;
  #playing = false;
  // How the current composition's load ended, or null while it is pending.
  #ready: { error: LumenPreviewError | null } | null = null;
  #readyWaiters: ((error: LumenPreviewError | null) => void)[] = [];
  // The part of the loaded timeline being played.
  #range: PlaybackRange | null = null;
  // Why the options last failed validation, and what was last said about a range that didn't fit.
  // The same message isn't reported again, so a callback that re-renders can't loop.
  #optionFailure: string | null = null;
  #rangeMisfit: string | null = null;
  // Why the inputs last failed to resolve. The same failure isn't reported again, so a callback that
  // re-renders with the same inputs can't loop.
  #resolveFailure: string | null = null;
  #seekId = 0;
  // The frame onTimeUpdate last reported, so a frame already shown isn't reported again.
  #reportedFrame: number | null = null;
  #startup: AbortController | null = null;
  #sentFonts: ReadonlyMap<string, MediaSourceInput> = new Map();
  #sentMedia: ReadonlyMap<string, PreviewMediaEntry> = new Map();
  #timeline: { fps: number; totalFrames: number } | null = null;
  readonly #wasmUrl: string | URL | null;
  #worker: Worker | null = null;
  readonly #workerUrl: string | URL | null;
  #workerOptions: PreviewWorkerOptions = {
    logLevel: "off",
    lookaheadCount: DEFAULT_LOOKAHEAD_COUNT,
  };

  constructor({
    audio,
    preview,
    runtimeUrl,
    wasmUrl,
    workerUrl,
    ...inputs
  }: LumenPreviewSessionOptions) {
    const runtimeFile = (name: string) => (runtimeUrl ? runtimeAsset(runtimeUrl, name) : null);
    const workerModuleUrl = audio?.workerModuleUrl ?? runtimeFile("audio-worker.js");
    const workletModuleUrl = audio?.workletModuleUrl ?? runtimeFile("audio-worklet.js");
    this.#audioOptions = {
      ...audio,
      ...(workerModuleUrl && { workerModuleUrl }),
      ...(workletModuleUrl && { workletModuleUrl }),
    };
    this.preview = preview;
    this.#wasmUrl = wasmUrl ?? runtimeFile("lumen_wasm_bg.wasm");
    this.#workerUrl = workerUrl ?? runtimeFile("worker.js");
    this.#inputs = inputs;
  }

  get loop(): boolean {
    return this.#playback.loop;
  }

  get muted(): boolean {
    return this.#playback.muted;
  }

  /** The range being played, or null for the whole composition. */
  get range(): LumenPreviewRange | null {
    const { range } = this.#playback;
    return range && { in: range.in, out: range.out };
  }

  get rate(): number {
    return this.#playback.rate;
  }

  get volume(): number {
    return this.#playback.volume;
  }

  /**
   * Resolves with null once the current composition has loaded with everything it needs, or with
   * the failure `onError` reported when it, or a source or font it needs, didn't load: an audio
   * source that can't be decoded is an `audio_failed` with its `media`, and the composition plays
   * without it. A change of composition, media or fonts before then waits for the new ones
   * instead. Without a composition it resolves at once. It never rejects.
   */
  whenReady(): Promise<LumenPreviewError | null> {
    if (this.#ready) {
      return Promise.resolve(this.#ready.error);
    }
    return new Promise((resolve) => this.#readyWaiters.push(resolve));
  }

  /** Replaces whatever this session was showing. Resolves when started or failed; never rejects. */
  async attach(host: HTMLElement): Promise<void> {
    this.#teardown();
    const attachment = ++this.#attachment;
    this.preview.update({
      error: null,
      errorCode: null,
      frame: 0,
      isLoaded: false,
      isPlaying: false,
      media: NO_MEDIA,
    });
    this.preview.attach({
      pause: () => this.#pause(),
      play: () => this.#play(),
      seek: (frame) => this.#seek(frame),
      whenReady: () => this.whenReady(),
    });

    let worker: Worker | null = null;
    this.#startup = new AbortController();
    try {
      assertBrowserSupport();
      if (this.#workerUrl === null || this.#wasmUrl === null) {
        throw new PreviewStartupError(
          "worker_failed",
          "The Lumen preview needs `runtimeUrl`, or both `workerUrl` and `wasmUrl`.",
        );
      }
      const workerUrl = absoluteUrl(this.#workerUrl);
      worker = createModuleWorker(workerUrl);
      this.#workerOptions = this.#desiredOptions();
      await startWorker(
        worker,
        {
          ...this.#workerOptions,
          type: "start",
          wasmUrl: absoluteUrl(this.#wasmUrl),
        },
        workerUrl,
        this.#startup.signal,
      );
    } catch (error) {
      worker?.terminate();
      if (attachment === this.#attachment) {
        const code = error instanceof PreviewStartupError ? error.code : "worker_failed";
        this.#settleReady(this.#reportError(code, "start", error));
      }
      return;
    }
    if (attachment !== this.#attachment) {
      worker.terminate();
      return;
    }

    const canvas = document.createElement("canvas");
    canvas.style.cssText = "display:block;width:100%;height:100%;object-fit:contain";
    host.append(canvas);
    const offscreen = canvas.transferControlToOffscreen();

    this.#canvas = canvas;
    this.#worker = worker;
    this.#audio = new LumenAudioEngine({
      ...this.#audioOptions,
      onClockChange: () => this.#correctClock(),
      onError: (message) => this.#reportError("audio_failed", "audio", message),
    });
    worker.onmessage = (event: MessageEvent<LumenPreviewWorkerEvent>) => this.#receive(event.data);
    worker.onerror = (event) => this.#workerCrashed(event.message);
    worker.onmessageerror = () => this.#workerCrashed("a message could not be read");
    this.#post({ canvas: offscreen, type: "attach-canvas" }, [offscreen]);
    this.#syncOptions();
    this.#applyPlayback(this.#readPlayback(this.#inputs));
    this.#syncInputs();
  }

  /**
   * Replaces every input; one left out returns to its default. Playback options that can't be used
   * reject the whole update, also while the worker is starting: it is reported through the
   * callbacks in effect, and everything in effect stays as it was.
   */
  update(inputs: LumenPreviewSessionInputs): void {
    const playback = this.#readPlayback(inputs);
    if (!playback) {
      return;
    }
    this.#inputs = inputs;
    // A session still starting applies these once its worker is ready.
    if (!this.#worker) {
      return;
    }
    this.#syncOptions();
    this.#applyPlayback(playback);
    this.#syncInputs();
  }

  dispose(): void {
    this.#attachment += 1;
    this.#teardown();
    this.#settleReady(stoppedError());
    this.preview.detach();
  }

  #teardown(): void {
    this.#startup?.abort();
    this.#startup = null;
    this.#stopCorrections();
    this.#settleSeeks(false);
    this.#audio?.dispose();
    this.#audio = null;
    this.#worker?.terminate();
    this.#worker = null;
    this.#canvas?.remove();
    this.#canvas = null;
    this.#clock = PARKED_CLOCK;
    this.#frameWaiting = false;
    this.#held = false;
    this.#loaded = null;
    this.#loadedId = 0;
    this.#shownId = 0;
    this.#loading = false;
    this.#media = null;
    this.#optionFailure = null;
    this.#pendingSeek = null;
    this.#playback = DEFAULT_PLAYBACK;
    this.#playbackRevision = 0;
    this.#playing = false;
    this.#range = null;
    this.#rangeMisfit = null;
    this.#reportedFrame = null;
    this.#resolveFailure = null;
    // The next worker counts its seeks from zero.
    this.#seekId = 0;
    this.#sentFonts = new Map();
    this.#sentMedia = new Map();
    this.#timeline = null;
  }

  #desiredOptions(): PreviewWorkerOptions {
    return {
      logLevel: this.#inputs.logLevel ?? "off",
      lookaheadCount: this.#inputs.lookaheadCount ?? DEFAULT_LOOKAHEAD_COUNT,
    };
  }

  #syncOptions(): void {
    const options = this.#desiredOptions();
    if (
      options.logLevel !== this.#workerOptions.logLevel ||
      options.lookaheadCount !== this.#workerOptions.lookaheadCount
    ) {
      this.#workerOptions = options;
      this.#post({ ...options, type: "set-options" });
    }
  }

  /** The playback options `inputs` ask for, or null after reporting why they can't be used. */
  #readPlayback(inputs: LumenPreviewSessionInputs): PlaybackSettings | null {
    let next: PlaybackSettings;
    try {
      next = readPlaybackSettings(inputs);
    } catch (error) {
      const message = describeError(error);
      if (message !== this.#optionFailure) {
        this.#optionFailure = message;
        this.#reportError("invalid_option", "options", error);
      }
      return null;
    }
    if (this.#optionFailure !== null) {
      this.#optionFailure = null;
      if (this.preview.getSnapshot().errorCode === "invalid_option") {
        this.preview.update({ error: null, errorCode: null });
      }
    }
    return next;
  }

  /** Applies the playback options that changed. */
  #applyPlayback(next: PlaybackSettings | null): void {
    if (!next) {
      return;
    }
    const previous = this.#playback;
    this.#playback = next;
    if (next.volume !== previous.volume || next.muted !== previous.muted) {
      this.#audio?.setVolume(next.volume, next.muted);
    }
    if (next.rate !== previous.rate) {
      this.#changeRate(next.rate);
    }
    if (next.loop !== previous.loop || !sameRange(next.range, previous.range)) {
      this.#changeRange();
    }
  }

  /** Playback carries on from where it is, at the new speed. */
  #changeRate(rate: number): void {
    const positionMs = this.#positionMs();
    this.#audio?.setRate(rate);
    if (!this.#running()) {
      return;
    }
    this.#playbackRevision += 1;
    this.#clock = this.#audio?.isPlaying()
      ? audioClock(this.#audio, rate)
      : { atEpochMs: epochNow(), originMs: positionMs, rate };
    this.#postClock(this.#clock);
  }

  /**
   * Playback carries on from the position it had under the old loop and range, which both clocks
   * are rebased to. A playhead outside the new range moves to its start.
   */
  #changeRange(): void {
    const timeline = this.#timeline;
    const positionMs = timeline ? this.#positionMs() : 0;
    const range = timeline ? this.#applyRange(timeline) : null;
    if (timeline) {
      this.#placeClock(positionMs);
    }
    const { loop, range: requested } = this.#playback;
    this.#playbackRevision += 1;
    this.#post({
      clock: this.#clock,
      loop,
      range: requested,
      revision: this.#playbackRevision,
      type: "set-playback",
    });
    if (!timeline || !range) {
      return;
    }
    const frame = frameAt(positionMs, timeline.fps);
    if (frame < range.inFrame || frame >= range.outFrame) {
      void this.#seek(range.inFrame);
    } else {
      this.#correctClock();
    }
  }

  /** Fits the requested range to the timeline, and tells the audio. */
  #applyRange(timeline: { fps: number; totalFrames: number }): PlaybackRange {
    const { loop, range: requested } = this.#playback;
    const misfit = describeRangeMisfit(requested, timeline.totalFrames);
    if (misfit !== null && misfit !== this.#rangeMisfit) {
      console.warn(`[LumenPreviewSession] ${misfit}`);
    }
    this.#rangeMisfit = misfit;
    const range = resolvePlaybackRange(requested, loop, timeline.totalFrames);
    this.#range = range;
    this.#audio?.setRange({
      endMs: (range.outFrame * 1_000) / timeline.fps,
      loop: range.loop,
      startMs: (range.inFrame * 1_000) / timeline.fps,
    });
    return range;
  }

  /** Loads only when something the worker reads changed, so unchanged inputs can be passed freely. */
  #syncInputs(): void {
    let next: ResolvedPreviewInputs;
    try {
      next = resolvePreviewInputs(this.#inputs.composition, this.#inputs.media, this.#inputs.fonts);
    } catch (error) {
      const message = describeError(error);
      if (message !== this.#resolveFailure) {
        this.#resolveFailure = message;
        this.#settleReady(this.#reportError("load_failed", "load", error));
      }
      return;
    }
    // Loading again replaces the resolution error with the loaded inputs' own outcome.
    const recovering = this.#resolveFailure !== null;
    this.#resolveFailure = null;

    const previous = this.#loaded;
    if (
      !recovering &&
      previous &&
      previous.compositionJson === next.compositionJson &&
      sameEntries(previous.media, next.media, sameMediaEntry) &&
      sameEntries(previous.fonts, next.fonts, sameMediaInput)
    ) {
      this.#loaded = next;
      return;
    }
    this.#load(next);
  }

  #load(next: ResolvedPreviewInputs): void {
    const loadId = ++this.#loadId;
    this.#loaded = next;
    this.#settleSeeks(false);
    this.#reportedFrame = null;
    this.#frameWaiting = false;
    this.#media = null;
    this.#ready = null;

    if (next.compositionJson === null) {
      this.#held = false;
      this.#loading = false;
      this.#pendingSeek = null;
      this.#playing = false;
      this.#stopCorrections();
      this.#clock = PARKED_CLOCK;
      this.#range = null;
      this.#timeline = null;
      this.#audio?.loadAudioTimeline(null);
      this.#post({ loadId, type: "unload" });
      this.preview.update({
        frame: 0,
        height: 0,
        isLoaded: false,
        isPlaying: false,
        media: NO_MEDIA,
        totalFrames: 0,
        width: 0,
      });
      this.#inputs.onStats?.(EMPTY_PREVIEW_STATS);
      this.#settleReady(null);
      return;
    }

    const media = [...next.media].filter(([source, entry]) => {
      const sent = this.#sentMedia.get(source);
      return !sent || !sameMediaEntry(sent, entry);
    });
    const fonts = [...next.fonts].filter(([family, source]) => {
      const sent = this.#sentFonts.get(family);
      return !sent || !sameMediaInput(sent, source);
    });
    this.#loading = true;
    try {
      this.#post({
        clock: this.#clock,
        compositionJson: next.compositionJson,
        fonts,
        keepFonts: [...next.fonts.keys()],
        keepMedia: [...next.media.keys()],
        loadId,
        media,
        type: "load",
      });
      this.#sentFonts = next.fonts;
      this.#sentMedia = next.media;
    } catch (error) {
      // A source that can't cross to a worker, such as a Request, throws here.
      this.#loadFailed(error);
    }
  }

  #loadFailed(
    error: unknown,
    diagnostics: readonly LumenDiagnostic[] = [],
    media: MediaFailure | null = errorMediaFailure(error),
  ): void {
    this.#loading = false;
    this.#pause();
    this.#range = null;
    this.#timeline = null;
    this.#inputs.onStats?.(EMPTY_PREVIEW_STATS);
    this.preview.update({ isLoaded: false });
    this.#settleReady(this.#reportError("load_failed", "load", error, diagnostics, media));
  }

  #receive(event: LumenPreviewWorkerEvent): void {
    switch (event.type) {
      case "loaded":
        this.#onLoaded(event);
        break;
      case "load-failed":
        if (event.loadId === this.#loadId) {
          this.#loadFailed(event.message, event.diagnostics, event.media);
        }
        break;
      case "frame":
        this.#onFrame(event);
        break;
      case "render-failed":
        if (event.loadId === this.#loadId) {
          this.#settleSeeks(false);
          this.#pause();
          const error = this.#reportError(
            "render_failed",
            "render",
            event.message,
            event.diagnostics,
            event.media,
          );
          if (!this.#ready) {
            this.#settleReady(error);
          }
        }
        break;
      case "waiting":
        if (event.loadId === this.#loadId) {
          this.#frameWaiting = event.waiting;
          this.#updateHold();
        }
        break;
      case "ended":
        if (
          event.loadId === this.#loadId &&
          event.seekId === this.#seekId &&
          event.playbackRevision === this.#playbackRevision
        ) {
          this.#onEnded();
        }
        break;
      case "media":
        if (event.loadId === this.#loadId) {
          this.#onMedia(event.media);
        }
        break;
      case "audio-source":
        this.#loadAudioSource(event);
        break;
      case "audio-source-released":
        this.#audio?.removeSource(event.source);
        break;
      case "ready":
      case "startup-failed":
        break;
    }
  }

  #onMedia(media: readonly MediaEntryState[]): void {
    this.#media = media;
    const count = (status: MediaEntryState["status"]) =>
      media.filter((entry) => entry.status === status).length;
    this.preview.update({
      media: { failed: count("failed"), pending: count("pending"), ready: count("ready") },
    });
    this.#updateHold();
    this.#checkReady();
  }

  /** Settles `whenReady` once a frame of the composition is shown and nothing it needs is pending. */
  #checkReady(): void {
    const media = this.#media;
    if (
      this.#ready ||
      this.#loadedId !== this.#loadId ||
      this.#shownId !== this.#loadId ||
      !media ||
      media.some(({ status }) => status === "pending")
    ) {
      return;
    }
    const failure = media.find((entry) => entry.failure)?.failure ?? null;
    this.#settleReady(
      failure && {
        code: "audio_failed",
        diagnostics: [],
        media: failure,
        message: `${failure.source}: ${failure.details}`,
      },
    );
  }

  #settleReady(error: LumenPreviewError | null): void {
    this.#ready = { error };
    const waiters = this.#readyWaiters;
    this.#readyWaiters = [];
    for (const resolve of waiters) {
      resolve(error);
    }
  }

  #loadAudioSource({ input, requestId, source }: WorkerEvent<"audio-source">): void {
    const settled = (failure: MediaFailure | null) =>
      this.#post({ failure, requestId, type: "audio-source-settled" });
    const audio = this.#audio;
    if (!audio) {
      return;
    }
    audio.loadSource(source, input).then(
      () => settled(null),
      (error: unknown) => {
        const failure = errorMediaFailure(error) ?? {
          code: "media.decode",
          details: describeError(error),
          source,
          wanted: ["audio"],
        };
        settled(failure);
        this.#reportError("audio_failed", "audio", error, [], failure);
      },
    );
  }

  /**
   * Playing waits, with its clock parked, while media the composition needs is pending or the
   * frame it is on is still loading.
   */
  #updateHold(): void {
    const hold =
      this.#playing &&
      (this.#frameWaiting || (this.#media?.some(({ status }) => status === "pending") ?? false));
    if (hold === this.#held) {
      return;
    }
    this.#held = hold;
    this.#playbackRevision += 1;
    if (hold) {
      const originMs = this.#positionMs();
      this.#audio?.pause();
      this.#stopCorrections();
      this.#clock = { atEpochMs: epochNow(), originMs, rate: 0 };
      if (this.#range && this.#timeline) {
        this.#clock = fitClock(this.#clock, this.#range, this.#timeline.fps);
      }
      this.#postClock(this.#clock);
    } else {
      this.#startClock();
    }
  }

  /**
   * The worker starts the new timeline on the clock it already follows, so an unchanged audio
   * timeline needs only the usual correction. A pending seek, or audio restarted for a new
   * timeline, seeks both.
   */
  #onLoaded(event: WorkerEvent<"loaded">): void {
    if (event.loadId !== this.#loadId) {
      return;
    }
    const { fps, totalFrames } = event;
    // Read against the previous timeline, before the audio can be told about the new one.
    const positionMs = this.#positionMs();
    const pending = this.#pendingSeek;
    this.#loadedId = event.loadId;
    this.#loading = false;
    this.#pendingSeek = null;
    this.#timeline = { fps, totalFrames };
    const audioChanged = this.#audio?.loadAudioTimeline(event.audioTimelineJson) ?? false;

    const range = this.#applyRange(this.#timeline);
    const lastFrame = range.outFrame - 1;
    const lastFrameMs = (lastFrame * 1_000) / fps;
    const restartAudio = audioChanged && this.#running();
    let frame = this.preview.getSnapshot().frame;
    if (pending !== null || restartAudio) {
      // Playback carries on round the new range, as the worker would.
      const originMs =
        pending !== null
          ? (clampFrame(pending, range) * 1_000) / fps
          : Math.min(rangeMs(positionMs, range, fps), lastFrameMs);
      if (restartAudio) {
        this.#audio?.play(originMs);
      } else {
        this.#audio?.seekMs(originMs);
      }
      this.#placeClock(originMs);
      this.#postSeek();
      frame = clampFrame(frameAt(originMs, fps), range);
    } else if (this.#running()) {
      this.#correctClock();
      // The frame on screen stands until the worker shows the next, unless it's outside the range.
      if (frame < range.inFrame || frame > lastFrame) {
        frame = rangeFrame(frameAt(this.#positionMs(), fps), range);
      }
    } else {
      // The worker fits its clock the same way.
      this.#clock = fitClock(this.#clock, range, fps);
      if (audioChanged) {
        this.#audio?.seekMs(this.#clock.originMs);
      }
      frame = frameAt(this.#clock.originMs, fps);
    }

    this.#inputs.onStats?.({
      ...EMPTY_PREVIEW_STATS,
      targetFrameDurationMs: 1_000 / fps,
      timelineFps: fps,
    });
    this.preview.update({
      // Newer inputs that didn't resolve keep their error over this earlier load.
      ...(this.#resolveFailure === null &&
        this.#optionFailure === null && { error: null, errorCode: null }),
      frame,
      height: event.height,
      isLoaded: true,
      isPlaying: this.#playing,
      totalFrames,
      width: event.width,
    });
    this.#checkReady();
  }

  #onFrame(event: WorkerEvent<"frame">): void {
    if (event.loadId !== this.#loadId || !this.#timeline) {
      return;
    }
    if (this.#shownId !== event.loadId) {
      this.#shownId = event.loadId;
      this.#checkReady();
    }
    for (const [seekId, resolve] of this.#pendingSeeks) {
      if (seekId <= event.seekId) {
        this.#pendingSeeks.delete(seekId);
        resolve(true);
      }
    }
    // This render started before a newer seek, so its frame would pull the playhead back.
    if (event.seekId !== this.#seekId) {
      return;
    }
    const recovered =
      this.preview.getSnapshot().errorCode === "render_failed"
        ? { error: null, errorCode: null }
        : {};
    this.preview.update({ frame: event.frame, ...recovered });
    // A seek to the frame on screen is answered without showing anything.
    if (event.frame !== this.#reportedFrame) {
      this.#reportedFrame = event.frame;
      this.#inputs.onTimeUpdate?.({
        frame: event.frame,
        timeMs: (event.frame * 1_000) / this.#timeline.fps,
      });
    }
    this.#inputs.onStats?.({
      actualFps: event.actualFps,
      frame: event.frame,
      renderMs: event.renderMs,
      targetFrameDurationMs: 1_000 / this.#timeline.fps,
      timelineFps: this.#timeline.fps,
    });
  }

  #play(): void {
    const range = this.#range;
    if (!this.#timeline || !range || this.#playing) {
      return;
    }
    // Playing from the last frame of a range that doesn't loop starts it again.
    if (!range.loop && frameAt(this.#positionMs(), this.#timeline.fps) >= range.outFrame - 1) {
      void this.#seek(range.inFrame);
    }
    this.#playing = true;
    this.#playbackRevision += 1;
    this.preview.update({ isPlaying: true });
    this.#updateHold();
    if (!this.#held) {
      this.#startClock();
    }
  }

  #startClock(): void {
    this.#audio?.play(this.#positionMs());
    this.#correctClock();
    this.#correctionTimer = setInterval(() => this.#correctClock(), CLOCK_CORRECTION_INTERVAL_MS);
  }

  /** Playing, and not held for media. */
  #running(): boolean {
    return this.#playing && !this.#held;
  }

  #pause(): void {
    if (!this.#playing) {
      return;
    }
    const originMs = this.#positionMs();
    this.#playing = false;
    this.#held = false;
    this.#playbackRevision += 1;
    this.#audio?.pause();
    this.#stopCorrections();
    this.#clock = { atEpochMs: epochNow(), originMs, rate: 0 };
    if (this.#range && this.#timeline) {
      this.#clock = fitClock(this.#clock, this.#range, this.#timeline.fps);
    }
    this.#postClock(this.#clock);
    this.preview.update({ isPlaying: false });
  }

  /** Playback ran out of range: it stops on the last frame, as a pause there would. */
  #onEnded(): void {
    const range = this.#range;
    const timeline = this.#timeline;
    if (!this.#playing || !range || !timeline) {
      return;
    }
    const lastFrame = range.outFrame - 1;
    const lastFrameMs = (lastFrame * 1_000) / timeline.fps;
    this.#playing = false;
    this.#audio?.pause();
    this.#audio?.seekMs(lastFrameMs);
    this.#stopCorrections();
    this.#clock = { atEpochMs: epochNow(), originMs: lastFrameMs, rate: 0 };
    this.#postClock(this.#clock);
    this.preview.update({ frame: lastFrame, isPlaying: false });
    this.#inputs.onEnded?.();
  }

  #seek(frame: number): Promise<boolean> {
    const timeline = this.#timeline;
    if (this.#loading || !timeline) {
      // The next load decides how long the composition is, then seeks there.
      this.#pendingSeek = Math.max(0, Math.round(frame));
      this.preview.update({ frame: this.#pendingSeek });
      return Promise.resolve(false);
    }
    const target = clampFrame(
      Math.max(0, Math.round(frame)),
      this.#range ?? { inFrame: 0, loop: false, outFrame: timeline.totalFrames },
    );
    const originMs = (target * 1_000) / timeline.fps;
    this.#audio?.seekMs(originMs);
    this.#placeClock(originMs);
    this.preview.update({ frame: target });
    const seekId = this.#postSeek();
    return new Promise((resolve) => this.#pendingSeeks.set(seekId, resolve));
  }

  #placeClock(originMs: number): void {
    this.#clock = {
      atEpochMs: epochNow(),
      originMs,
      rate: this.#running() && !this.#audio?.isPlaying() ? this.#playback.rate : 0,
    };
  }

  #postSeek(): number {
    const seekId = ++this.#seekId;
    this.#post({ clock: this.#clock, seekId, type: "seek" });
    return seekId;
  }

  /** Audio leads whenever it's playing. Without it the worker's clock simply keeps running. */
  #correctClock(): void {
    if (!this.#running()) {
      return;
    }
    const audio = this.#audio;
    let next: PreviewClock | null = null;
    if (audio?.isPlaying()) {
      next = audioClock(audio, this.#playback.rate);
    } else if (this.#clock.rate === 0) {
      next = { atEpochMs: epochNow(), originMs: this.#clock.originMs, rate: this.#playback.rate };
    }
    if (next) {
      this.#clock = next;
      this.#postClock(next);
    }
  }

  #postClock(clock: PreviewClock): void {
    this.#post({ clock, revision: this.#playbackRevision, type: "clock" });
  }

  /** Wrapped into the range, or held at its end, as the worker shows it. */
  #positionMs(): number {
    const ms = this.#audio?.isPlaying()
      ? this.#audio.currentTimeMs()
      : clockPositionMs(this.#clock, epochNow());
    return this.#range && this.#timeline
      ? rangeMs(Math.max(ms, 0), this.#range, this.#timeline.fps)
      : Math.max(ms, 0);
  }

  #stopCorrections(): void {
    if (this.#correctionTimer !== null) {
      clearInterval(this.#correctionTimer);
      this.#correctionTimer = null;
    }
  }

  #settleSeeks(presented: boolean): void {
    for (const resolve of this.#pendingSeeks.values()) {
      resolve(presented);
    }
    this.#pendingSeeks.clear();
  }

  #workerCrashed(message: string): void {
    this.#teardown();
    this.preview.update({ isLoaded: false, isPlaying: false, media: NO_MEDIA });
    this.#settleReady(
      this.#reportError("worker_failed", "worker", `the preview worker stopped: ${message}`),
    );
  }

  #reportError(
    code: LumenPreviewErrorCode,
    scope: string,
    error: unknown,
    diagnostics: readonly LumenDiagnostic[] = [],
    media: MediaFailure | null = errorMediaFailure(error),
  ): LumenPreviewError {
    reportConsoleError(scope, error);
    const message = describeError(error);
    const reported = { code, diagnostics, media, message };
    this.preview.update({ error: message, errorCode: code });
    this.#inputs.onError?.(reported);
    return reported;
  }

  #post(message: LumenPreviewWorkerCommand, transfer: Transferable[] = []): void {
    this.#worker?.postMessage(message, transfer);
  }
}

export function mountPreview(
  host: HTMLElement,
  options: LumenPreviewSessionOptions,
): LumenPreviewSession {
  const session = new LumenPreviewSession(options);
  void session.attach(host);
  return session;
}

/**
 * `name` in the directory `runtimeUrl` names, which it is whether or not its path ends with a
 * slash. Its query is kept on every file, and its fragment dropped.
 */
function runtimeAsset(runtimeUrl: string | URL, name: string): string {
  const base = new URL(absoluteUrl(runtimeUrl));
  if (!base.pathname.endsWith("/")) {
    base.pathname += "/";
  }
  const url = new URL(name, base);
  url.search = base.search;
  return url.href;
}

function stoppedError(): LumenPreviewError {
  return {
    code: "load_failed",
    diagnostics: [],
    media: null,
    message: "the preview stopped before its composition loaded",
  };
}

function assertBrowserSupport(): void {
  if (
    !("gpu" in navigator) ||
    typeof OffscreenCanvas === "undefined" ||
    typeof HTMLCanvasElement.prototype.transferControlToOffscreen !== "function"
  ) {
    throw new PreviewStartupError(
      "webgpu_unavailable",
      "Lumen preview needs WebGPU and OffscreenCanvas, which this browser or page doesn't provide. WebGPU also needs https or localhost.",
    );
  }
}

/** Waits for the worker to prove it can render, before anything irreversible happens to a canvas. */
function startWorker(
  worker: Worker,
  start: Extract<LumenPreviewWorkerCommand, { type: "start" }>,
  workerUrl: string,
  signal: AbortSignal,
): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const failed = (detail: string) =>
      reject(
        new PreviewStartupError(
          "worker_failed",
          `The Lumen preview worker failed to start from ${workerUrl}: ${detail}. Check that the page's CSP allows it (worker-src, script-src 'wasm-unsafe-eval'). ${assetHint(workerUrl)}`,
        ),
      );
    const timeout = setTimeout(
      () => failed(`no answer after ${WORKER_STARTUP_TIMEOUT_MS / 1_000} s`),
      WORKER_STARTUP_TIMEOUT_MS,
    );
    signal.addEventListener("abort", () => {
      clearTimeout(timeout);
      worker.terminate();
      reject(new DOMException("the preview stopped while starting", "AbortError"));
    });
    worker.onmessage = (event: MessageEvent<LumenPreviewWorkerEvent>) => {
      if (event.data.type === "ready") {
        clearTimeout(timeout);
        resolve();
      } else if (event.data.type === "startup-failed") {
        clearTimeout(timeout);
        reject(new PreviewStartupError(event.data.code, event.data.message));
      }
    };
    worker.onerror = (event) => {
      clearTimeout(timeout);
      failed(event.message || "the worker script didn't load");
    };
    worker.onmessageerror = () => {
      clearTimeout(timeout);
      failed("a message could not be read");
    };
    worker.postMessage(start);
  });
}

function audioClock(audio: LumenAudioEngine, rate: number): PreviewClock {
  return {
    atEpochMs: epochNow(),
    originMs: audio.currentTimeMs(),
    rate: audio.isClockRunning() ? rate : 0,
  };
}

/** A seek lands inside the range, whether or not it loops. */
function clampFrame(frame: number, range: PlaybackRange): number {
  return Math.max(range.inFrame, Math.min(frame, range.outFrame - 1));
}

function sameRange(left: LumenPreviewRange | null, right: LumenPreviewRange | null): boolean {
  return left?.in === right?.in && left?.out === right?.out;
}

// Matches the worker's rounding, so a frame's own start time doesn't land on the frame before.
function frameAt(positionMs: number, fps: number): number {
  return Math.floor((positionMs * fps) / 1_000 + 1e-6);
}

function sameEntries<T>(
  left: ReadonlyMap<string, T>,
  right: ReadonlyMap<string, T>,
  same: (left: T, right: T) => boolean,
): boolean {
  if (left.size !== right.size) {
    return false;
  }
  for (const [key, value] of left) {
    const other = right.get(key);
    if (other === undefined || !same(value, other)) {
      return false;
    }
  }
  return true;
}
