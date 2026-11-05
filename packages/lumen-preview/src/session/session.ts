import { LumenAudioEngine } from "../audio-engine.js";
import { clockPositionMs, epochNow, type PreviewClock } from "../clock.js";
import type { LumenWorkerBindings } from "../index.js";
import type { MediaRegistration, MediaSourceInput } from "../media/index.js";
import { sameMediaInput } from "../media/source.js";
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
  readonly #bindings: LumenWorkerBindings;
  #canvas: HTMLCanvasElement | null = null;
  #clock = PARKED_CLOCK;
  #correctionTimer: ReturnType<typeof setInterval> | null = null;
  #inputs: LumenPreviewSessionInputs;
  #loadId = 0;
  // What was last asked for, whether or not it worked: only a change is worth trying again.
  #loaded: ResolvedPreviewInputs | null = null;
  #pendingSeeks = new Map<number, (presented: boolean) => void>();
  #playing = false;
  #seekId = 0;
  #startup: AbortController | null = null;
  #sentFonts: ReadonlyMap<string, MediaSourceInput> = new Map();
  #sentMedia: ReadonlyMap<string, MediaRegistration> = new Map();
  #timeline: { fps: number; totalFrames: number } | null = null;
  #worker: Worker | null = null;
  #workerOptions: PreviewWorkerOptions = {
    logLevel: "off",
    lookaheadCount: DEFAULT_LOOKAHEAD_COUNT,
  };

  constructor({ audio, bindings, preview, ...inputs }: LumenPreviewSessionOptions) {
    this.#audioOptions = audio;
    this.#bindings = bindings;
    this.preview = preview;
    this.#inputs = inputs;
  }

  /** Replaces whatever this session was showing. Resolves when started or failed; never rejects. */
  async attach(host: HTMLElement): Promise<void> {
    this.#teardown();
    const attachment = ++this.#attachment;
    this.preview.update({ error: null, errorCode: null, isLoaded: false, isPlaying: false });

    let worker: Worker | null = null;
    this.#startup = new AbortController();
    try {
      assertBrowserSupport();
      assertWorkerBindings(this.#bindings);
      worker = new Worker(String(this.#bindings.previewWorkerUrl()), { type: "module" });
      this.#workerOptions = this.#desiredOptions();
      await startWorker(
        worker,
        {
          ...this.#workerOptions,
          type: "start",
          wasmUrl: absoluteUrl(this.#bindings.previewWasmUrl()),
        },
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
    this.#playing = false;
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
      this.#reportError("load_failed", "load", error);
      return;
    }

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
    this.#playing = false;
    this.#stopCorrections();
    this.#settleSeeks(false);
    this.#timeline = null;
    this.#audio?.loadAudioTimeline(null);

    if (next.compositionJson === null) {
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
    this.preview.update({ isPlaying: false });
    try {
      this.#post({
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
      this.#reportError("load_failed", "load", error);
    }
  }

  #receive(event: LumenPreviewWorkerEvent): void {
    switch (event.type) {
      case "loaded":
        this.#onLoaded(event);
        break;
      case "load-failed":
        if (event.loadId === this.#loadId) {
          this.#inputs.onStats?.(EMPTY_PREVIEW_STATS);
          this.preview.update({ isLoaded: false });
          this.#reportError("load_failed", "load", event.message);
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

  #onLoaded(event: WorkerEvent<"loaded">): void {
    if (event.loadId !== this.#loadId) {
      return;
    }
    this.#clock = PARKED_CLOCK;
    this.#timeline = { fps: event.fps, totalFrames: event.totalFrames };
    this.#audio?.loadAudioTimeline(event.audioTimelineJson);
    this.#syncAudioSources();
    this.#inputs.onStats?.({
      ...EMPTY_PREVIEW_STATS,
      targetFrameDurationMs: 1_000 / event.fps,
      timelineFps: event.fps,
    });
    this.preview.update({
      error: null,
      errorCode: null,
      frame: 0,
      height: event.height,
      isLoaded: true,
      isPlaying: false,
      totalFrames: event.totalFrames,
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
    if (!timeline) {
      this.preview.update({ frame });
      return Promise.resolve(false);
    }
    const target = Math.max(0, Math.min(Math.round(frame), timeline.totalFrames - 1));
    const originMs = (target * 1_000) / timeline.fps;
    this.#audio?.seekMs(originMs);
    this.#clock = {
      atEpochMs: epochNow(),
      originMs,
      rate: this.#playing && !this.#audio?.isPlaying() ? 1 : 0,
    };
    const seekId = ++this.#seekId;
    this.#post({ clock: this.#clock, seekId, type: "seek" });
    this.preview.update({ frame: target });
    return new Promise((resolve) => this.#pendingSeeks.set(seekId, resolve));
  }

  /** Audio leads whenever it's playing. Without it the worker's clock simply keeps running. */
  #correctClock(): void {
    if (!this.#playing) {
      return;
    }
    const atEpochMs = epochNow();
    const audio = this.#audio;
    let next: PreviewClock | null = null;
    if (audio?.isPlaying()) {
      next = {
        atEpochMs,
        originMs: audio.currentTimeMs(),
        rate: audio.isClockRunning() ? 1 : 0,
      };
    } else if (this.#clock.rate === 0) {
      next = { atEpochMs, originMs: this.#clock.originMs, rate: 1 };
    }
    if (next) {
      this.#clock = next;
      this.#post({ clock: next, type: "clock" });
    }
  }

  #positionMs(): number {
    return clockPositionMs(this.#clock, epochNow());
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

  #reportError(code: LumenPreviewErrorCode, scope: string, error: unknown): void {
    reportConsoleError(scope, error);
    this.preview.update({ error: describeError(error), errorCode: code });
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

function assertWorkerBindings(bindings: LumenWorkerBindings): void {
  if (!bindings.previewWorkerUrl?.() || !bindings.previewWasmUrl?.()) {
    throw new PreviewStartupError(
      "worker_failed",
      "Lumen preview needs bindings that name a worker and its wasm. Use createLumenBindings from @lumiscia/lumen-bindings, or /bundler under Vite.",
    );
  }
}

/** Waits for the worker to prove it can render, before anything irreversible happens to a canvas. */
function startWorker(
  worker: Worker,
  start: Extract<LumenPreviewWorkerCommand, { type: "start" }>,
  signal: AbortSignal,
): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const failed = (detail: string) =>
      reject(
        new PreviewStartupError(
          "worker_failed",
          `The Lumen preview worker failed to start: ${detail}. Check that the worker and wasm URLs are served and that the page's CSP allows them (worker-src, script-src 'wasm-unsafe-eval').`,
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
