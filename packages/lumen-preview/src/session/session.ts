import type { Diagnostic, MediaFailure } from "@lumiscia/lumen-types";

import { LumenAudioEngine } from "../audio-engine.js";
import {
  DEFAULT_PLAYBACK,
  readPlaybackSettings,
  type LumenPreviewRange,
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
import { Attachment, type PreviewWorkerOptions } from "./attachment.js";
import { describeError, errorMediaFailure, reportConsoleError } from "./errors.js";
import {
  absoluteUrl,
  resolvePreviewInputs,
  sameMediaEntry,
  type ResolvedPreviewInputs,
} from "./inputs.js";
import { Load } from "./load.js";
import { EMPTY_PREVIEW_STATS } from "./stats.js";
import type {
  LumenPreviewSessionInputs,
  LumenPreviewSessionOptions,
  LumenRuntime,
  LumenRuntimeFile,
} from "./types.js";

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
  #loadCount = 0;
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
    const attachment = new Attachment(this.#desiredOptions(), (state) =>
      this.#previewInternals.update(state),
    );
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
      pause: () => this.#live?.transport.pause(),
      play: () => this.#live?.transport.play(),
      seek: (frame) => this.#seek(frame),
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
    attachment.transport.audio = new LumenAudioEngine({
      ...this.#audioOptions,
      workerUrl: this.#runtime.audioWorker,
      workletUrl: this.#runtime.audioWorklet,
      onClockChange: () => attachment.transport.correctClock(),
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
    return this.#live?.transport.playback ?? DEFAULT_PLAYBACK;
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
    const { transport } = live;
    const previous = transport.playback;
    if (next.volume !== previous.volume || next.muted !== previous.muted) {
      transport.audio?.setVolume(next.volume, next.muted);
    }
    transport.setPlayback(next);
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

    const previous = live.load?.inputs;
    if (
      !recovering &&
      live.load &&
      previous &&
      previous.compositionJson === next.compositionJson &&
      sameEntries(previous.media, next.media, sameMediaEntry) &&
      sameEntries(previous.fonts, next.fonts, sameMediaInput)
    ) {
      live.load.inputs = next;
      return;
    }
    this.#load(live, next);
  }

  #load(live: Attachment, next: ResolvedPreviewInputs): void {
    const load = new Load(++this.#loadCount, next);
    live.load = load;
    live.transport.beginLoad();
    live.reportedFrame = null;
    this.#ready = null;

    const { compositionJson } = next;
    if (compositionJson === null) {
      live.transport.unload();
      live.post({ loadId: load.id, type: "unload" });
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
    try {
      live.post({
        clock: live.transport.clock,
        compositionJson,
        fonts,
        keepFonts: [...next.fonts.keys()],
        keepMedia: [...next.media.keys()],
        loadId: load.id,
        media,
        type: "load",
      });
      live.sentFonts = next.fonts;
      live.sentMedia = next.media;
    } catch (error) {
      // A source that can't cross to a worker, such as a Request, throws here.
      this.#loadFailed(live, load, error);
    }
  }

  #loadFailed(
    live: Attachment,
    load: Load,
    error: unknown,
    diagnostics: readonly Diagnostic[] = [],
    media: MediaFailure | null = errorMediaFailure(error),
  ): void {
    load.failed();
    live.transport.failLoad();
    this.#inputs.onStats?.(EMPTY_PREVIEW_STATS);
    this.#previewInternals.update({ isLoaded: false });
    this.#settleReady(this.#reportError("load_failed", "load", error, diagnostics, media));
  }

  #receive(event: LumenPreviewWorkerEvent): void {
    const live = this.#live;
    if (!live) {
      return;
    }
    const { load, transport } = live;
    switch (event.type) {
      case "loaded":
        this.#onLoaded(live, event);
        break;
      case "load-failed":
        if (load?.id === event.loadId) {
          this.#loadFailed(live, load, event.message, event.diagnostics, event.media);
        }
        break;
      case "frame":
        this.#onFrame(live, event);
        break;
      case "render-failed":
        if (load?.id === event.loadId) {
          transport.settleSeeks(false);
          transport.pause();
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
        if (load?.id === event.loadId) {
          transport.setFrameWaiting(event.waiting);
        }
        break;
      case "ended":
        if (load?.id === event.loadId && transport.ended(event.seekId, event.playbackRevision)) {
          this.#inputs.onEnded?.();
        }
        break;
      case "media":
        if (load?.id === event.loadId) {
          this.#onMedia(live, load, event.media);
        }
        break;
      case "audio-source":
        this.#loadAudioSource(live, event);
        break;
      case "audio-source-released":
        transport.audio?.removeSource(event.source);
        break;
      case "ready":
      case "startup-failed":
        break;
    }
  }

  #onMedia(live: Attachment, load: Load, media: readonly MediaEntryState[]): void {
    load.media = media;
    const count = (status: MediaEntryState["status"]) =>
      media.filter((entry) => entry.status === status).length;
    this.#previewInternals.update({
      media: { failed: count("failed"), pending: count("pending"), ready: count("ready") },
    });
    live.transport.updateHold();
    this.#checkReady(load);
  }

  /** Settles `whenReady` once a frame of the composition is shown and nothing it needs is pending. */
  #checkReady(load: Load): void {
    const media = load.media;
    // A subscriber may have replaced the load while this one's event was being handled.
    if (this.#live?.load !== load || this.#ready || !load.isShown || !media || load.mediaPending) {
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
    { transport }: Attachment,
    { input, requestId, source }: WorkerEvent<"audio-source">,
  ): void {
    const settled = (failure: MediaFailure | null) =>
      this.#live?.post({ failure, requestId, type: "audio-source-settled" });
    const audio = transport.audio;
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

  #onLoaded(live: Attachment, event: WorkerEvent<"loaded">): void {
    const { load, transport } = live;
    if (load?.id !== event.loadId) {
      return;
    }
    load.loaded();
    const frame = transport.load(event, this.preview.getSnapshot().frame);
    this.#inputs.onStats?.({
      ...EMPTY_PREVIEW_STATS,
      targetFrameDurationMs: 1_000 / event.fps,
      timelineFps: event.fps,
    });
    this.#previewInternals.update({
      // Newer inputs that didn't resolve keep their error over this earlier load.
      ...(live.resolveFailure === null &&
        this.#optionFailure === null && { error: null, errorCode: null }),
      frame,
      height: event.height,
      isLoaded: true,
      isPlaying: transport.isPlaying,
      totalFrames: event.totalFrames,
      width: event.width,
    });
    this.#checkReady(load);
  }

  #onFrame(live: Attachment, event: WorkerEvent<"frame">): void {
    const { load, transport } = live;
    const timeline = transport.timeline;
    if (load?.id !== event.loadId || !timeline) {
      return;
    }
    if (load.shown()) {
      this.#checkReady(load);
    }
    // This render started before a newer seek, so its frame would pull the playhead back.
    if (!transport.acknowledgeSeeks(event.seekId)) {
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
        timeMs: (event.frame * 1_000) / timeline.fps,
      });
    }
    this.#inputs.onStats?.({
      actualFps: event.actualFps,
      frame: event.frame,
      renderMs: event.renderMs,
      targetFrameDurationMs: 1_000 / timeline.fps,
      timelineFps: timeline.fps,
    });
  }

  /** The preview still offers its seek once the worker has crashed, though there is nothing to seek. */
  #seek(frame: number): Promise<boolean> {
    if (this.#live) {
      return this.#live.transport.seek(frame);
    }
    this.#previewInternals.update({ frame: Math.max(0, Math.round(frame)) });
    return Promise.resolve(false);
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
