import type { LumenDiagnostic } from "@lumiscia/lumen-types";

import { LumenAudioEngine } from "../audio-engine.js";
import { clockPositionMs, epochNow, fitClock, type PreviewClock } from "../clock.js";
import type { MediaRegistration, MediaSourceInput } from "../media/index.js";
import { sameMediaInput } from "../media/source.js";
import { assetHint } from "../asset-hint.js";
import { createModuleWorker } from "../module-worker.js";
import type { LumenPreviewContext, LumenPreviewErrorCode } from "../preview.js";
import type { LumenPreviewWorkerCommand, LumenPreviewWorkerEvent } from "../worker-host.js";
import { describeError, reportConsoleError } from "./errors.js";
import {
  absoluteUrl,
  resolvePreviewInputs,
  sameAudioSources,
  sameRegistration,
  type ResolvedPreviewInputs,
} from "./inputs.js";
import { EMPTY_PREVIEW_STATS } from "./stats.js";
import type { LumenPreviewSessionInputs, LumenPreviewSessionOptions } from "./types.js";

const CLOCK_CORRECTION_INTERVAL_MS = 250;
const DEFAULT_LOOKAHEAD_COUNT = 8;
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
  #inputs: LumenPreviewSessionInputs;
  #loadId = 0;
  // Whether a load is in flight. A seek waits for it, to be clamped to the new length.
  #loading = false;
  // What was last asked for, whether or not it worked: only a change is worth trying again.
  #loaded: ResolvedPreviewInputs | null = null;
  // A seek made without a loaded timeline, applied once one loads.
  #pendingSeek: number | null = null;
  #pendingSeeks = new Map<number, (presented: boolean) => void>();
  #playing = false;
  // Why the inputs last failed to resolve. The same failure isn't reported again, so a callback that
  // re-renders with the same inputs can't loop.
  #resolveFailure: string | null = null;
  #seekId = 0;
  #startup: AbortController | null = null;
  #sentFonts: ReadonlyMap<string, MediaSourceInput> = new Map();
  #sentMedia: ReadonlyMap<string, MediaRegistration> = new Map();
  #timeline: { fps: number; totalFrames: number } | null = null;
  readonly #wasmUrl: string | URL;
  #worker: Worker | null = null;
  readonly #workerUrl: string | URL;
  #workerOptions: PreviewWorkerOptions = {
    logLevel: "off",
    lookaheadCount: DEFAULT_LOOKAHEAD_COUNT,
  };

  constructor({ audio, preview, wasmUrl, workerUrl, ...inputs }: LumenPreviewSessionOptions) {
    this.#audioOptions = audio;
    this.preview = preview;
    this.#wasmUrl = wasmUrl;
    this.#workerUrl = workerUrl;
    this.#inputs = inputs;
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
    });

    let worker: Worker | null = null;
    this.#startup = new AbortController();
    try {
      assertBrowserSupport();
      worker = createModuleWorker(this.#workerUrl);
      this.#workerOptions = this.#desiredOptions();
      await startWorker(
        worker,
        {
          ...this.#workerOptions,
          type: "start",
          wasmUrl: absoluteUrl(this.#wasmUrl),
        },
        absoluteUrl(this.#workerUrl),
        this.#startup.signal,
      );
    } catch (error) {
      worker?.terminate();
      if (attachment === this.#attachment) {
        const code = error instanceof PreviewStartupError ? error.code : "worker_failed";
        this.#reportError(code, "start", error);
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
    this.preview.attach({
      pause: () => this.#pause(),
      play: () => this.#play(),
      seek: (frame) => this.#seek(frame),
    });
    this.#syncInputs();
  }

  /** Replaces every input; one left out returns to its default. */
  update(inputs: LumenPreviewSessionInputs): void {
    this.#inputs = inputs;
    // A session still starting applies these once its worker is ready.
    if (this.#worker) {
      this.#syncOptions();
      this.#syncInputs();
    }
  }

  dispose(): void {
    this.#attachment += 1;
    this.#teardown();
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
    this.#loaded = null;
    this.#loading = false;
    this.#pendingSeek = null;
    this.#playing = false;
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

  /** Loads only when something the worker reads changed, so unchanged inputs can be passed freely. */
  #syncInputs(): void {
    let next: ResolvedPreviewInputs;
    try {
      next = resolvePreviewInputs(this.#inputs.composition, this.#inputs.media, this.#inputs.fonts);
    } catch (error) {
      const message = describeError(error);
      if (message !== this.#resolveFailure) {
        this.#resolveFailure = message;
        this.#reportError("load_failed", "load", error);
      }
      return;
    }
    this.#resolveFailure = null;

    const previous = this.#loaded;
    if (
      previous &&
      previous.compositionJson === next.compositionJson &&
      sameEntries(previous.media, next.media, sameRegistration) &&
      sameEntries(previous.fonts, next.fonts, sameMediaInput)
    ) {
      this.#loaded = next;
      if (this.#timeline && !sameAudioSources(previous.audio, next.audio)) {
        this.#syncAudioSources();
      }
      return;
    }
    this.#load(next);
  }

  #load(next: ResolvedPreviewInputs): void {
    const loadId = ++this.#loadId;
    this.#loaded = next;
    this.#settleSeeks(false);

    if (next.compositionJson === null) {
      this.#loading = false;
      this.#pendingSeek = null;
      this.#playing = false;
      this.#stopCorrections();
      this.#clock = PARKED_CLOCK;
      this.#timeline = null;
      this.#audio?.loadAudioTimeline(null);
      this.#post({ loadId, type: "unload" });
      this.preview.update({
        frame: 0,
        height: 0,
        isLoaded: false,
        isPlaying: false,
        totalFrames: 0,
        width: 0,
      });
      this.#inputs.onStats?.(EMPTY_PREVIEW_STATS);
      return;
    }

    const media = [...next.media.values()].filter((registration) => {
      const sent = this.#sentMedia.get(registration.id);
      return !sent || !sameRegistration(sent, registration);
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

  #loadFailed(error: unknown, diagnostics: readonly LumenDiagnostic[] = []): void {
    this.#loading = false;
    this.#pause();
    this.#timeline = null;
    this.#inputs.onStats?.(EMPTY_PREVIEW_STATS);
    this.preview.update({ isLoaded: false });
    this.#reportError("load_failed", "load", error, diagnostics);
  }

  #receive(event: LumenPreviewWorkerEvent): void {
    switch (event.type) {
      case "loaded":
        this.#onLoaded(event);
        break;
      case "load-failed":
        if (event.loadId === this.#loadId) {
          this.#loadFailed(event.message, event.diagnostics);
        }
        break;
      case "frame":
        this.#onFrame(event);
        break;
      case "render-failed":
        if (event.loadId === this.#loadId) {
          this.#settleSeeks(false);
          this.#pause();
          this.#reportError("render_failed", "render", event.message);
        }
        break;
      case "ready":
      case "startup-failed":
        break;
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
    this.#loading = false;
    this.#pendingSeek = null;
    this.#timeline = { fps, totalFrames };
    const audioChanged = this.#audio?.loadAudioTimeline(event.audioTimelineJson) ?? false;
    this.#syncAudioSources();

    const lastFrame = Math.max(totalFrames - 1, 0);
    const loopMs = (totalFrames * 1_000) / fps;
    const restartAudio = audioChanged && this.#playing;
    let frame = this.preview.getSnapshot().frame;
    if (pending !== null || restartAudio) {
      // Playback carries on round the new loop, as the worker would.
      const originMs =
        pending !== null
          ? (Math.min(pending, lastFrame) * 1_000) / fps
          : loopMs > 0
            ? positionMs % loopMs
            : 0;
      if (restartAudio) {
        this.#audio?.play(originMs);
      } else {
        this.#audio?.seekMs(originMs);
      }
      this.#placeClock(originMs);
      this.#postSeek();
      frame = Math.min(frameAt(originMs, fps), lastFrame);
    } else if (this.#playing) {
      this.#correctClock();
      // The frame on screen stands until the worker shows the next, unless it's past the new end.
      if (frame > lastFrame) {
        frame = Math.min(frameAt(this.#positionMs(), fps), lastFrame);
      }
    } else {
      // The worker fits its clock the same way.
      this.#clock = fitClock(this.#clock, totalFrames, fps);
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
      error: null,
      errorCode: null,
      frame,
      height: event.height,
      isLoaded: true,
      isPlaying: this.#playing,
      totalFrames,
      width: event.width,
    });
  }

  #onFrame(event: WorkerEvent<"frame">): void {
    if (event.loadId !== this.#loadId || !this.#timeline) {
      return;
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
    this.#inputs.onStats?.({
      actualFps: event.actualFps,
      frame: event.frame,
      renderMs: event.renderMs,
      targetFrameDurationMs: 1_000 / this.#timeline.fps,
      timelineFps: this.#timeline.fps,
    });
  }

  #play(): void {
    if (!this.#timeline || this.#playing) {
      return;
    }
    this.#playing = true;
    this.#audio?.play(this.#positionMs());
    this.#correctClock();
    this.#correctionTimer = setInterval(() => this.#correctClock(), CLOCK_CORRECTION_INTERVAL_MS);
    this.preview.update({ isPlaying: true });
  }

  #pause(): void {
    if (!this.#playing) {
      return;
    }
    const originMs = this.#positionMs();
    this.#playing = false;
    this.#audio?.pause();
    this.#stopCorrections();
    this.#clock = { atEpochMs: epochNow(), originMs, rate: 0 };
    this.#post({ clock: this.#clock, type: "clock" });
    this.preview.update({ isPlaying: false });
  }

  #seek(frame: number): Promise<boolean> {
    const timeline = this.#timeline;
    if (this.#loading || !timeline) {
      // The next load decides how long the composition is, then seeks there.
      this.#pendingSeek = Math.max(0, Math.round(frame));
      this.preview.update({ frame: this.#pendingSeek });
      return Promise.resolve(false);
    }
    const target = Math.max(0, Math.min(Math.round(frame), timeline.totalFrames - 1));
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
      rate: this.#playing && !this.#audio?.isPlaying() ? 1 : 0,
    };
  }

  #postSeek(): number {
    const seekId = ++this.#seekId;
    this.#post({ clock: this.#clock, seekId, type: "seek" });
    return seekId;
  }

  /** Audio leads whenever it's playing. Without it the worker's clock simply keeps running. */
  #correctClock(): void {
    if (!this.#playing) {
      return;
    }
    const audio = this.#audio;
    let next: PreviewClock | null = null;
    if (audio?.isPlaying()) {
      next = audioClock(audio);
    } else if (this.#clock.rate === 0) {
      next = { atEpochMs: epochNow(), originMs: this.#clock.originMs, rate: 1 };
    }
    if (next) {
      this.#clock = next;
      this.#post({ clock: next, type: "clock" });
    }
  }

  /** Wrapped into the timeline, as the worker loops over it. */
  #positionMs(): number {
    const ms = this.#audio?.isPlaying()
      ? this.#audio.currentTimeMs()
      : clockPositionMs(this.#clock, epochNow());
    const loopMs = this.#timeline ? (this.#timeline.totalFrames * 1_000) / this.#timeline.fps : 0;
    return loopMs > 0 ? Math.max(ms, 0) % loopMs : Math.max(ms, 0);
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

  #syncAudioSources(): void {
    void this.#audio?.syncAudioSources(this.#loaded?.audio ?? []).catch((error: unknown) => {
      this.#reportError("audio_failed", "audio", error);
    });
  }

  #workerCrashed(message: string): void {
    this.#teardown();
    this.preview.update({ isLoaded: false, isPlaying: false });
    this.#reportError("worker_failed", "worker", `the preview worker stopped: ${message}`);
  }

  #reportError(
    code: LumenPreviewErrorCode,
    scope: string,
    error: unknown,
    diagnostics: readonly LumenDiagnostic[] = [],
  ): void {
    reportConsoleError(scope, error);
    const message = describeError(error);
    this.preview.update({ error: message, errorCode: code });
    this.#inputs.onError?.({ code, message, diagnostics });
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

function audioClock(audio: LumenAudioEngine): PreviewClock {
  return {
    atEpochMs: epochNow(),
    originMs: audio.currentTimeMs(),
    rate: audio.isClockRunning() ? 1 : 0,
  };
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
