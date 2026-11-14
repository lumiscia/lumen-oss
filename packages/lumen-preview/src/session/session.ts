import type { Diagnostic, MediaFailure } from "@lumiscia/lumen-types";

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
import type { MediaEntryState } from "../media/lifecycle.js";
import { sameMediaInput } from "../media/source.js";
import { assetHint } from "../asset-hint.js";
import { createModuleWorker } from "../module-worker.js";
import { internalsOf, type LumenPreviewInternals } from "../preview-internals.js";
import type {
  LumenPreviewContext,
  LumenPreviewError,
  LumenPreviewErrorCode,
  LumenPreviewMediaState,
} from "../preview.js";
import type { LumenPreviewWorkerCommand, LumenPreviewWorkerEvent } from "../worker-host.js";
import { Attachment, PARKED_CLOCK, type PreviewWorkerOptions } from "./attachment.js";
import { describeError, errorMediaFailure, reportConsoleError } from "./errors.js";
import {
  absoluteUrl,
  resolvePreviewInputs,
  sameMediaEntry,
  type ResolvedPreviewInputs,
} from "./inputs.js";
import { EMPTY_PREVIEW_STATS } from "./stats.js";
import type {
  LumenPreviewSessionInputs,
  LumenPreviewSessionOptions,
  LumenRuntime,
  LumenRuntimeFile,
} from "./types.js";

const CLOCK_CORRECTION_INTERVAL_MS = 250;
const DEFAULT_LOOKAHEAD_COUNT = 8;
const NO_MEDIA: LumenPreviewMediaState = { failed: 0, pending: 0, ready: 0 };
const WORKER_STARTUP_TIMEOUT_MS = 30_000;

type WorkerEvent<TType extends LumenPreviewWorkerEvent["type"]> = Extract<
  LumenPreviewWorkerEvent,
  { type: TType }
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
  readonly #previewInternals: LumenPreviewInternals;
  readonly #audioOptions: LumenPreviewSessionOptions["audio"];
  readonly #runtime: RuntimeUrls;
  #inputs: LumenPreviewSessionInputs;
  // What this session is attached with. Every attach replaces it and dispose drops it, so a slow
  // startup can tell it has been replaced.
  #live: Attachment | null = null;
  // Counts across attachments: a load is named the same to the worker that sees it and to the page.
  #loadId = 0;
  // Why the options last failed validation. The same message isn't reported again, so a callback
  // that re-renders can't loop. Setting inputs reads it also while nothing is attached.
  #optionFailure: string | null = null;
  // How the current composition's load ended, or null while it is pending.
  #ready: { error: LumenPreviewError | null } | null = null;
  #readyWaiters: ((error: LumenPreviewError | null) => void)[] = [];

  constructor({ audio, preview, runtime, runtimeUrl, ...inputs }: LumenPreviewSessionOptions) {
    this.#audioOptions = audio;
    this.#runtime = resolveRuntime(runtime, runtimeUrl);
    this.preview = preview;
    this.#previewInternals = internalsOf(preview);
    this.#inputs = inputs;
  }

  get loop(): boolean {
    return this.#playback().loop;
  }

  get muted(): boolean {
    return this.#playback().muted;
  }

  /** The range being played, or null for the whole composition. */
  get range(): LumenPreviewRange | null {
    const { range } = this.#playback();
    return range && { in: range.in, out: range.out };
  }

  get rate(): number {
    return this.#playback().rate;
  }

  get volume(): number {
    return this.#playback().volume;
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
    const attachment = new Attachment(this.#desiredOptions());
    this.#live = attachment;
    this.#previewInternals.update({
      error: null,
      errorCode: null,
      frame: 0,
      isLoaded: false,
      isPlaying: false,
      media: NO_MEDIA,
    });
    this.#previewInternals.attach({
      pause: () => {
        if (this.#live) {
          this.#pause(this.#live);
        }
      },
      play: () => {
        if (this.#live) {
          this.#play(this.#live);
        }
      },
      seek: (frame) => this.#seek(this.#live, frame),
      whenReady: () => this.whenReady(),
    });

    let worker: Worker | null = null;
    try {
      assertBrowserSupport();
      worker = createModuleWorker(this.#runtime.worker);
      await startWorker(
        worker,
        {
          ...attachment.workerOptions,
          type: "start",
          wasmUrl: this.#runtime.wasm,
        },
        this.#runtime.worker,
        attachment.startup.signal,
      );
    } catch (error) {
      worker?.terminate();
      if (attachment === this.#live) {
        const code = error instanceof PreviewStartupError ? error.code : "worker_failed";
        this.#settleReady(this.#reportError(code, "start", error));
      }
      return;
    }
    if (attachment !== this.#live) {
      worker.terminate();
      return;
    }

    const canvas = document.createElement("canvas");
    canvas.style.cssText = "display:block;width:100%;height:100%;object-fit:contain";
    host.append(canvas);
    const offscreen = canvas.transferControlToOffscreen();

    attachment.canvas = canvas;
    attachment.worker = worker;
    attachment.audio = new LumenAudioEngine({
      ...this.#audioOptions,
      workerUrl: this.#runtime.audioWorker,
      workletUrl: this.#runtime.audioWorklet,
      onClockChange: () => this.#correctClock(attachment),
      onError: (message) => this.#reportError("audio_failed", "audio", message),
    });
    worker.onmessage = (event: MessageEvent<LumenPreviewWorkerEvent>) => this.#receive(event.data);
    worker.onerror = (event) => this.#workerCrashed(event.message);
    worker.onmessageerror = () => this.#workerCrashed("a message could not be read");
    attachment.post({ canvas: offscreen, type: "attach-canvas" }, [offscreen]);
    this.#syncOptions(attachment);
    this.#applyPlayback(attachment, this.#readPlayback(this.#inputs));
    this.#syncInputs(attachment);
  }

  /**
   * Replaces every input: one left out returns to its default, so pass the whole set each time.
   * Inputs that are unchanged do nothing, so it is safe to call with them on every render.
   * Playback options that can't be used reject the whole call, also while the worker is starting:
   * it is reported through the callbacks in effect, and everything in effect stays as it was.
   */
  setInputs(inputs: LumenPreviewSessionInputs): void {
    const playback = this.#readPlayback(inputs);
    if (!playback) {
      return;
    }
    this.#inputs = inputs;
    // A session still starting applies these once its worker is ready.
    const live = this.#live;
    if (!live?.worker) {
      return;
    }
    this.#syncOptions(live);
    this.#applyPlayback(live, playback);
    this.#syncInputs(live);
  }

  dispose(): void {
    this.#teardown();
    this.#settleReady(stoppedError());
    this.#previewInternals.detach();
  }

  #teardown(): void {
    this.#live?.dispose();
    this.#live = null;
    this.#optionFailure = null;
  }

  /** The playback options in effect: the defaults until a worker is ready to apply the inputs'. */
  #playback(): PlaybackSettings {
    return this.#live?.playback ?? DEFAULT_PLAYBACK;
  }

  #desiredOptions(): PreviewWorkerOptions {
    return {
      logLevel: this.#inputs.logLevel ?? "off",
      lookaheadCount: this.#inputs.lookaheadCount ?? DEFAULT_LOOKAHEAD_COUNT,
    };
  }

  #syncOptions(live: Attachment): void {
    const options = this.#desiredOptions();
    if (
      options.logLevel !== live.workerOptions.logLevel ||
      options.lookaheadCount !== live.workerOptions.lookaheadCount
    ) {
      live.workerOptions = options;
      live.post({ ...options, type: "set-options" });
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
        this.#previewInternals.update({ error: null, errorCode: null });
      }
    }
    return next;
  }

  /** Applies the playback options that changed. */
  #applyPlayback(live: Attachment, next: PlaybackSettings | null): void {
    if (!next) {
      return;
    }
    const previous = live.playback;
    live.playback = next;
    if (next.volume !== previous.volume || next.muted !== previous.muted) {
      live.audio?.setVolume(next.volume, next.muted);
    }
    if (next.rate !== previous.rate) {
      this.#changeRate(live, next.rate);
    }
    if (next.loop !== previous.loop || !sameRange(next.range, previous.range)) {
      this.#changeRange(live);
    }
  }

  /** Playback carries on from where it is, at the new speed. */
  #changeRate(live: Attachment, rate: number): void {
    const positionMs = this.#positionMs(live);
    live.audio?.setRate(rate);
    if (!live.running) {
      return;
    }
    live.playbackRevision += 1;
    live.clock = live.audio?.isPlaying()
      ? audioClock(live.audio, rate)
      : { atEpochMs: epochNow(), originMs: positionMs, rate };
    this.#postClock(live, live.clock);
  }

  /**
   * Playback carries on from the position it had under the old loop and range, which both clocks
   * are rebased to. A playhead outside the new range moves to its start.
   */
  #changeRange(live: Attachment): void {
    const timeline = live.timeline;
    const positionMs = timeline ? this.#positionMs(live) : 0;
    const range = timeline ? this.#applyRange(live, timeline) : null;
    if (timeline) {
      this.#placeClock(live, positionMs);
    }
    const { loop, range: requested } = live.playback;
    live.playbackRevision += 1;
    live.post({
      clock: live.clock,
      loop,
      range: requested,
      revision: live.playbackRevision,
      type: "set-playback",
    });
    if (!timeline || !range) {
      return;
    }
    const frame = frameAt(positionMs, timeline.fps);
    if (frame < range.inFrame || frame >= range.outFrame) {
      void this.#seek(live, range.inFrame);
    } else {
      this.#correctClock(live);
    }
  }

  /** Fits the requested range to the timeline, and tells the audio. */
  #applyRange(live: Attachment, timeline: { fps: number; totalFrames: number }): PlaybackRange {
    const { loop, range: requested } = live.playback;
    const misfit = describeRangeMisfit(requested, timeline.totalFrames);
    if (misfit !== null && misfit !== live.rangeMisfit) {
      console.warn(`[LumenPreviewSession] ${misfit}`);
    }
    live.rangeMisfit = misfit;
    const range = resolvePlaybackRange(requested, loop, timeline.totalFrames);
    live.range = range;
    live.audio?.setRange({
      endMs: (range.outFrame * 1_000) / timeline.fps,
      loop: range.loop,
      startMs: (range.inFrame * 1_000) / timeline.fps,
    });
    return range;
  }

  /** Reads the composition again and loads only when the JSON or another input the worker reads changed. */
  #syncInputs(live: Attachment): void {
    let next: ResolvedPreviewInputs;
    try {
      next = resolvePreviewInputs(this.#inputs.composition, this.#inputs.media, this.#inputs.fonts);
    } catch (error) {
      const message = describeError(error);
      if (message !== live.resolveFailure) {
        live.resolveFailure = message;
        this.#settleReady(this.#reportError("load_failed", "load", error));
      }
      return;
    }
    // Loading again replaces the resolution error with the loaded inputs' own outcome.
    const recovering = live.resolveFailure !== null;
    live.resolveFailure = null;

    const previous = live.loaded;
    if (
      !recovering &&
      previous &&
      previous.compositionJson === next.compositionJson &&
      sameEntries(previous.media, next.media, sameMediaEntry) &&
      sameEntries(previous.fonts, next.fonts, sameMediaInput)
    ) {
      live.loaded = next;
      return;
    }
    this.#load(live, next);
  }

  #load(live: Attachment, next: ResolvedPreviewInputs): void {
    const loadId = ++this.#loadId;
    live.loaded = next;
    live.settleSeeks(false);
    live.reportedFrame = null;
    live.frameWaiting = false;
    live.media = null;
    this.#ready = null;

    if (next.compositionJson === null) {
      live.held = false;
      live.loading = false;
      live.pendingSeek = null;
      live.playing = false;
      live.stopCorrections();
      live.clock = PARKED_CLOCK;
      live.range = null;
      live.timeline = null;
      live.audio?.loadAudioTimeline(null);
      live.post({ loadId, type: "unload" });
      this.#previewInternals.update({
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
      const sent = live.sentMedia.get(source);
      return !sent || !sameMediaEntry(sent, entry);
    });
    const fonts = [...next.fonts].filter(([family, source]) => {
      const sent = live.sentFonts.get(family);
      return !sent || !sameMediaInput(sent, source);
    });
    live.loading = true;
    try {
      live.post({
        clock: live.clock,
        compositionJson: next.compositionJson,
        fonts,
        keepFonts: [...next.fonts.keys()],
        keepMedia: [...next.media.keys()],
        loadId,
        media,
        type: "load",
      });
      live.sentFonts = next.fonts;
      live.sentMedia = next.media;
    } catch (error) {
      // A source that can't cross to a worker, such as a Request, throws here.
      this.#loadFailed(live, error);
    }
  }

  #loadFailed(
    live: Attachment,
    error: unknown,
    diagnostics: readonly Diagnostic[] = [],
    media: MediaFailure | null = errorMediaFailure(error),
  ): void {
    live.loading = false;
    this.#pause(live);
    live.range = null;
    live.timeline = null;
    this.#inputs.onStats?.(EMPTY_PREVIEW_STATS);
    this.#previewInternals.update({ isLoaded: false });
    this.#settleReady(this.#reportError("load_failed", "load", error, diagnostics, media));
  }

  #receive(event: LumenPreviewWorkerEvent): void {
    const live = this.#live;
    if (!live) {
      return;
    }
    switch (event.type) {
      case "loaded":
        this.#onLoaded(live, event);
        break;
      case "load-failed":
        if (event.loadId === this.#loadId) {
          this.#loadFailed(live, event.message, event.diagnostics, event.media);
        }
        break;
      case "frame":
        this.#onFrame(live, event);
        break;
      case "render-failed":
        if (event.loadId === this.#loadId) {
          live.settleSeeks(false);
          this.#pause(live);
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
          live.frameWaiting = event.waiting;
          this.#updateHold(live);
        }
        break;
      case "ended":
        if (
          event.loadId === this.#loadId &&
          event.seekId === live.seekId &&
          event.playbackRevision === live.playbackRevision
        ) {
          this.#onEnded(live);
        }
        break;
      case "media":
        if (event.loadId === this.#loadId) {
          this.#onMedia(live, event.media);
        }
        break;
      case "audio-source":
        this.#loadAudioSource(live, event);
        break;
      case "audio-source-released":
        live.audio?.removeSource(event.source);
        break;
      case "ready":
      case "startup-failed":
        break;
    }
  }

  #onMedia(live: Attachment, media: readonly MediaEntryState[]): void {
    live.media = media;
    const count = (status: MediaEntryState["status"]) =>
      media.filter((entry) => entry.status === status).length;
    this.#previewInternals.update({
      media: { failed: count("failed"), pending: count("pending"), ready: count("ready") },
    });
    this.#updateHold(live);
    this.#checkReady(live);
  }

  /** Settles `whenReady` once a frame of the composition is shown and nothing it needs is pending. */
  #checkReady(live: Attachment): void {
    const media = live.media;
    if (
      this.#ready ||
      live.loadedId !== this.#loadId ||
      live.shownId !== this.#loadId ||
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

  #loadAudioSource(
    live: Attachment,
    { input, requestId, source }: WorkerEvent<"audio-source">,
  ): void {
    const settled = (failure: MediaFailure | null) =>
      this.#live?.post({ failure, requestId, type: "audio-source-settled" });
    const audio = live.audio;
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
  #updateHold(live: Attachment): void {
    const hold =
      live.playing &&
      (live.frameWaiting || (live.media?.some(({ status }) => status === "pending") ?? false));
    if (hold === live.held) {
      return;
    }
    live.held = hold;
    live.playbackRevision += 1;
    if (hold) {
      const originMs = this.#positionMs(live);
      live.audio?.pause();
      live.stopCorrections();
      live.clock = { atEpochMs: epochNow(), originMs, rate: 0 };
      if (live.range && live.timeline) {
        live.clock = fitClock(live.clock, live.range, live.timeline.fps);
      }
      this.#postClock(live, live.clock);
    } else {
      this.#startClock(live);
    }
  }

  /**
   * The worker starts the new timeline on the clock it already follows, so an unchanged audio
   * timeline needs only the usual correction. A pending seek, or audio restarted for a new
   * timeline, seeks both.
   */
  #onLoaded(live: Attachment, event: WorkerEvent<"loaded">): void {
    if (event.loadId !== this.#loadId) {
      return;
    }
    const { fps, totalFrames } = event;
    // Read against the previous timeline, before the audio can be told about the new one.
    const positionMs = this.#positionMs(live);
    const pending = live.pendingSeek;
    live.loadedId = event.loadId;
    live.loading = false;
    live.pendingSeek = null;
    const timeline = { fps, totalFrames };
    live.timeline = timeline;
    const audioChanged =
      live.audio?.loadAudioTimeline(event.audioTimelineJson, {
        durationFrames: totalFrames,
        fps,
      }) ?? false;

    const range = this.#applyRange(live, timeline);
    const lastFrame = range.outFrame - 1;
    const lastFrameMs = (lastFrame * 1_000) / fps;
    const restartAudio = audioChanged && live.running;
    let frame = this.preview.getSnapshot().frame;
    if (pending !== null || restartAudio) {
      // Playback carries on round the new range, as the worker would.
      const originMs =
        pending !== null
          ? (clampFrame(pending, range) * 1_000) / fps
          : Math.min(rangeMs(positionMs, range, fps), lastFrameMs);
      if (restartAudio) {
        live.audio?.play(originMs);
      } else {
        live.audio?.seekMs(originMs);
      }
      this.#placeClock(live, originMs);
      this.#postSeek(live);
      frame = clampFrame(frameAt(originMs, fps), range);
    } else if (live.running) {
      this.#correctClock(live);
      // The frame on screen stands until the worker shows the next, unless it's outside the range.
      if (frame < range.inFrame || frame > lastFrame) {
        frame = rangeFrame(frameAt(this.#positionMs(live), fps), range);
      }
    } else {
      // The worker fits its clock the same way.
      live.clock = fitClock(live.clock, range, fps);
      if (audioChanged) {
        live.audio?.seekMs(live.clock.originMs);
      }
      frame = frameAt(live.clock.originMs, fps);
    }

    this.#inputs.onStats?.({
      ...EMPTY_PREVIEW_STATS,
      targetFrameDurationMs: 1_000 / fps,
      timelineFps: fps,
    });
    this.#previewInternals.update({
      // Newer inputs that didn't resolve keep their error over this earlier load.
      ...(live.resolveFailure === null &&
        this.#optionFailure === null && { error: null, errorCode: null }),
      frame,
      height: event.height,
      isLoaded: true,
      isPlaying: live.playing,
      totalFrames,
      width: event.width,
    });
    this.#checkReady(live);
  }

  #onFrame(live: Attachment, event: WorkerEvent<"frame">): void {
    if (event.loadId !== this.#loadId || !live.timeline) {
      return;
    }
    if (live.shownId !== event.loadId) {
      live.shownId = event.loadId;
      this.#checkReady(live);
    }
    for (const [seekId, resolve] of live.pendingSeeks) {
      if (seekId <= event.seekId) {
        live.pendingSeeks.delete(seekId);
        resolve(true);
      }
    }
    // This render started before a newer seek, so its frame would pull the playhead back.
    if (event.seekId !== live.seekId) {
      return;
    }
    const recovered =
      this.preview.getSnapshot().errorCode === "render_failed"
        ? { error: null, errorCode: null }
        : {};
    this.#previewInternals.update({ frame: event.frame, ...recovered });
    // A seek to the frame on screen is answered without showing anything.
    if (event.frame !== live.reportedFrame) {
      live.reportedFrame = event.frame;
      this.#inputs.onTimeUpdate?.({
        frame: event.frame,
        timeMs: (event.frame * 1_000) / live.timeline.fps,
      });
    }
    this.#inputs.onStats?.({
      actualFps: event.actualFps,
      frame: event.frame,
      renderMs: event.renderMs,
      targetFrameDurationMs: 1_000 / live.timeline.fps,
      timelineFps: live.timeline.fps,
    });
  }

  #play(live: Attachment): void {
    const range = live.range;
    if (!live.timeline || !range || live.playing) {
      return;
    }
    // Playing from the last frame of a range that doesn't loop starts it again.
    if (!range.loop && frameAt(this.#positionMs(live), live.timeline.fps) >= range.outFrame - 1) {
      void this.#seek(live, range.inFrame);
    }
    live.playing = true;
    live.playbackRevision += 1;
    this.#previewInternals.update({ isPlaying: true });
    this.#updateHold(live);
    if (!live.held) {
      this.#startClock(live);
    }
  }

  #startClock(live: Attachment): void {
    live.audio?.play(this.#positionMs(live));
    this.#correctClock(live);
    live.correctionTimer = setInterval(
      () => this.#correctClock(live),
      CLOCK_CORRECTION_INTERVAL_MS,
    );
  }

  #pause(live: Attachment): void {
    if (!live.playing) {
      return;
    }
    const originMs = this.#positionMs(live);
    live.playing = false;
    live.held = false;
    live.playbackRevision += 1;
    live.audio?.pause();
    live.stopCorrections();
    live.clock = { atEpochMs: epochNow(), originMs, rate: 0 };
    if (live.range && live.timeline) {
      live.clock = fitClock(live.clock, live.range, live.timeline.fps);
    }
    this.#postClock(live, live.clock);
    this.#previewInternals.update({ isPlaying: false });
  }

  /** Playback ran out of range: it stops on the last frame, as a pause there would. */
  #onEnded(live: Attachment): void {
    const range = live.range;
    const timeline = live.timeline;
    if (!live.playing || !range || !timeline) {
      return;
    }
    const lastFrame = range.outFrame - 1;
    const lastFrameMs = (lastFrame * 1_000) / timeline.fps;
    live.playing = false;
    live.audio?.pause();
    live.audio?.seekMs(lastFrameMs);
    live.stopCorrections();
    live.clock = { atEpochMs: epochNow(), originMs: lastFrameMs, rate: 0 };
    this.#postClock(live, live.clock);
    this.#previewInternals.update({ frame: lastFrame, isPlaying: false });
    this.#inputs.onEnded?.();
  }

  /** `live` is null once the worker has crashed, while the preview still offers its seek. */
  #seek(live: Attachment | null, frame: number): Promise<boolean> {
    const timeline = live?.timeline;
    if (!live || live.loading || !timeline) {
      // The next load decides how long the composition is, then seeks there.
      const pending = Math.max(0, Math.round(frame));
      if (live) {
        live.pendingSeek = pending;
      }
      this.#previewInternals.update({ frame: pending });
      return Promise.resolve(false);
    }
    const target = clampFrame(
      Math.max(0, Math.round(frame)),
      live.range ?? { inFrame: 0, loop: false, outFrame: timeline.totalFrames },
    );
    const originMs = (target * 1_000) / timeline.fps;
    live.audio?.seekMs(originMs);
    this.#placeClock(live, originMs);
    this.#previewInternals.update({ frame: target });
    const seekId = this.#postSeek(live);
    return new Promise((resolve) => live.pendingSeeks.set(seekId, resolve));
  }

  #placeClock(live: Attachment, originMs: number): void {
    live.clock = {
      atEpochMs: epochNow(),
      originMs,
      rate: live.running && !live.audio?.isPlaying() ? live.playback.rate : 0,
    };
  }

  #postSeek(live: Attachment): number {
    const seekId = ++live.seekId;
    live.post({ clock: live.clock, seekId, type: "seek" });
    return seekId;
  }

  /** Audio leads whenever it's playing. Without it the worker's clock simply keeps running. */
  #correctClock(live: Attachment): void {
    if (!live.running) {
      return;
    }
    const audio = live.audio;
    let next: PreviewClock | null = null;
    if (audio?.isPlaying()) {
      next = audioClock(audio, live.playback.rate);
    } else if (live.clock.rate === 0) {
      next = { atEpochMs: epochNow(), originMs: live.clock.originMs, rate: live.playback.rate };
    }
    if (next) {
      live.clock = next;
      this.#postClock(live, next);
    }
  }

  #postClock(live: Attachment, clock: PreviewClock): void {
    live.post({ clock, revision: live.playbackRevision, type: "clock" });
  }

  /** Wrapped into the range, or held at its end, as the worker shows it. */
  #positionMs(live: Attachment): number {
    const ms = live.audio?.isPlaying()
      ? live.audio.currentTimeMs()
      : clockPositionMs(live.clock, epochNow());
    return live.range && live.timeline
      ? rangeMs(Math.max(ms, 0), live.range, live.timeline.fps)
      : Math.max(ms, 0);
  }

  #workerCrashed(message: string): void {
    this.#teardown();
    this.#previewInternals.update({ isLoaded: false, isPlaying: false, media: NO_MEDIA });
    this.#settleReady(
      this.#reportError("worker_failed", "worker", `the preview worker stopped: ${message}`),
    );
  }

  #reportError(
    code: LumenPreviewErrorCode,
    scope: string,
    error: unknown,
    diagnostics: readonly Diagnostic[] = [],
    media: MediaFailure | null = errorMediaFailure(error),
  ): LumenPreviewError {
    reportConsoleError(scope, error);
    const message = describeError(error);
    const reported = { code, diagnostics, media, message };
    this.#previewInternals.update({ error: message, errorCode: code });
    this.#inputs.onError?.(reported);
    return reported;
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

interface RuntimeUrls {
  worker: string;
  wasm: string;
  audioWorker: string;
  audioWorklet: string;
}

/** Where each runtime file loads from: its `file` in `runtimeUrl` when that is given, else its `url`. */
function resolveRuntime(
  runtime: LumenRuntime | undefined,
  runtimeUrl: string | URL | undefined,
): RuntimeUrls {
  if (!runtime) {
    throw new TypeError(
      "The Lumen preview needs `runtime`: `runtime` from `@lumiscia/lumen-bindings/urls`, or `runtime2d` from `@lumiscia/lumen-bindings/urls-2d`.",
    );
  }
  const locate = ({ file, url }: LumenRuntimeFile) =>
    runtimeUrl === undefined ? absoluteUrl(url) : runtimeFileUrl(runtimeUrl, file);
  return {
    worker: locate(runtime.worker),
    wasm: locate(runtime.wasm),
    audioWorker: locate(runtime.audioWorker),
    audioWorklet: locate(runtime.audioWorklet),
  };
}

/**
 * `name` in the directory `runtimeUrl` names, which it is whether or not its path ends with a
 * slash. Its query is kept on every file, and its fragment dropped.
 */
function runtimeFileUrl(runtimeUrl: string | URL, name: string): string {
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
