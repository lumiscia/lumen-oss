import type { LumenDiagnostic, MediaFailure } from "@lumiscia/lumen-types";
import { epochNow, type PreviewClock } from "./clock.js";
import {
  previewDependencies,
  samePreviewMediaInput,
  type PreviewMediaEntry,
  type PreviewMediaInput,
} from "./media/dependencies.js";
import { toMediaError } from "./media/errors.js";
import { MediaLifecycle, type MediaEntryState, type MediaLoader } from "./media/lifecycle.js";
import {
  createLumenPreviewRuntime,
  type LumenLogLevel,
  type LumenPreviewBindings,
  type LumenPreviewController,
} from "./runtime.js";
import type { MediaSourceInput } from "./media/index.js";
import { sourceInputToBlob } from "./media/source.js";
import type { LumenPreviewErrorCode } from "./preview.js";
import { resolvePlaybackRange, type LumenPreviewRange } from "./playback-range.js";
import { PreviewRunner } from "./runner.js";
import { assetHint } from "./asset-hint.js";
import { describeError, errorDiagnostics, errorMediaFailure } from "./session/errors.js";
import { PlaybackFpsMeter } from "./session/stats.js";

export interface LumenPreviewWorkerHostOptions {
  initPreview: (options: { module_or_path: string }) => Promise<object>;
  previewBindings: LumenPreviewBindings;
}

export type LumenPreviewWorkerCommand =
  | {
      logLevel: LumenLogLevel;
      lookaheadCount: number;
      type: "start";
      wasmUrl: string;
    }
  | {
      canvas: OffscreenCanvas;
      type: "attach-canvas";
    }
  | {
      /** Where playback is. The first render follows it, and so do clock changes during the load. */
      clock: PreviewClock;
      compositionJson: string;
      /** New or changed fonts. A source can be large, so one the worker has isn't sent again. */
      fonts: readonly (readonly [family: string, source: MediaSourceInput])[];
      /** Every family and id this load uses, including those sent by earlier loads. */
      keepFonts: readonly string[];
      keepMedia: readonly string[];
      loadId: number;
      /** New or changed media, as with `fonts`. */
      media: readonly (readonly [source: string, entry: PreviewMediaEntry])[];
      type: "load";
    }
  | {
      /** Null once the page has the audio, or why it couldn't decode it. */
      failure: MediaFailure | null;
      requestId: number;
      type: "audio-source-settled";
    }
  | {
      loadId: number;
      type: "unload";
    }
  | {
      clock: PreviewClock;
      /** The playback revision this clock belongs to. */
      revision: number;
      type: "clock";
    }
  | {
      clock: PreviewClock;
      seekId: number;
      type: "seek";
    }
  | {
      logLevel: LumenLogLevel;
      lookaheadCount: number;
      type: "set-options";
    }
  | {
      /** The same position under the new loop and range, so time counted under the old ones isn't reread. */
      clock: PreviewClock;
      loop: boolean;
      /** Null plays the whole timeline. */
      range: LumenPreviewRange | null;
      /** Counts changes that make a queued `ended` stale; `ended` events say which they ended under. */
      revision: number;
      type: "set-playback";
    };

export type LumenPreviewWorkerEvent =
  | {
      type: "ready";
    }
  | {
      code: LumenPreviewErrorCode;
      message: string;
      type: "startup-failed";
    }
  | {
      audioTimelineJson: string | null;
      fps: number;
      height: number;
      loadId: number;
      totalFrames: number;
      type: "loaded";
      width: number;
    }
  | {
      diagnostics: LumenDiagnostic[];
      loadId: number;
      /** The media source that couldn't be used, when that is why the load failed. */
      media: MediaFailure | null;
      message: string;
      type: "load-failed";
    }
  | {
      actualFps: number;
      frame: number;
      loadId: number;
      renderMs: number;
      /** The newest seek this frame answers. */
      seekId: number;
      type: "frame";
    }
  | {
      loadId: number;
      /** The media source that couldn't be used, when that is why the frame failed. */
      media: MediaFailure | null;
      message: string;
      type: "render-failed";
    }
  | {
      loadId: number;
      /** Each dependency of the composition being loaded or shown, and how far it has got. */
      media: MediaEntryState[];
      type: "media";
    }
  | {
      /** The audio of `source`, which the page decodes, and answers with `audio-source-settled`. */
      input: MediaSourceInput;
      requestId: number;
      source: string;
      type: "audio-source";
    }
  | {
      source: string;
      type: "audio-source-released";
    }
  | {
      loadId: number;
      /** The playback revision the playback ended under. */
      playbackRevision: number;
      /** The newest seek the worker had heard, so a late answer can't end a playback begun since. */
      seekId: number;
      type: "ended";
    };

type PreviewWorkerScope = typeof globalThis & {
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<LumenPreviewWorkerCommand>) => void,
  ): void;
  postMessage(message: LumenPreviewWorkerEvent): void;
};

type WorkerNavigator = Navigator & { gpu?: { requestAdapter(): Promise<unknown> } };

/** Runs inside the preview worker: it holds the wasm controller and renders what the page asks. */
export function createLumenPreviewWorkerHost({
  initPreview,
  previewBindings,
}: LumenPreviewWorkerHostOptions): void {
  const scope = globalThis as PreviewWorkerScope;
  const audioRequests = new Map<number, (failure: MediaFailure | null) => void>();
  const fonts = new Map<string, MediaSourceInput>();
  const media = new Map<string, PreviewMediaEntry>();
  const playbackFps = new PlaybackFpsMeter();
  // The newest clock the page sent, which a load starts on.
  let clock: PreviewClock = { atEpochMs: 0, originMs: 0, rate: 0 };
  let audioRequestId = 0;
  let canvas: OffscreenCanvas | null = null;
  let controller: LumenPreviewController | null = null;
  let lifecycle: MediaLifecycle<PreviewMediaInput> | null = null;
  let durationFrames = 0;
  let lastRenderMs = 0;
  let loadId = 0;
  let loop = false;
  let playbackRevision = 0;
  let range: LumenPreviewRange | null = null;

  const runner = new PreviewRunner<ReturnType<typeof setTimeout>>(
    {
      prepareFrame: () => requireController().prepareFrame(),
      renderFrame: () => {
        if (!canvas) {
          throw new Error("the preview worker has no canvas");
        }
        return requireController().renderPrepared(canvas);
      },
      setFrame: (frame) => requireController().setFrame(frame),
    },
    {
      clearTimer: (timer) => clearTimeout(timer),
      now: epochNow,
      onEnded: (seekId) => {
        scope.postMessage({ loadId, playbackRevision, seekId, type: "ended" });
      },
      onFailure: (error) => {
        scope.postMessage({
          loadId,
          media: errorMediaFailure(error),
          message: describeError(error),
          type: "render-failed",
        });
      },
      onFrame: (frame, seekId, renderMs) => {
        lastRenderMs = renderMs ?? lastRenderMs;
        scope.postMessage({
          actualFps: playbackFps.sample(frame, clock.rate > 0),
          frame,
          loadId,
          renderMs: lastRenderMs,
          seekId,
          type: "frame",
        });
      },
      setTimer: (callback, delayMs) => setTimeout(callback, delayMs),
    },
  );

  scope.addEventListener("message", (event) => {
    void handleMessage(event.data);
  });

  async function handleMessage(message: LumenPreviewWorkerCommand): Promise<void> {
    switch (message.type) {
      case "start":
        await start(message);
        break;
      case "attach-canvas":
        canvas = message.canvas;
        break;
      case "load":
        await load(message);
        break;
      case "unload":
        loadId = message.loadId;
        durationFrames = 0;
        runner.unload();
        requireLifecycle().clear();
        requireController().clear();
        break;
      case "audio-source-settled":
        audioRequests.get(message.requestId)?.(message.failure);
        audioRequests.delete(message.requestId);
        break;
      case "clock":
        followClock(message.clock);
        playbackRevision = message.revision;
        runner.setClock(message.clock);
        break;
      case "seek":
        followClock(message.clock);
        runner.seek(message.seekId, message.clock);
        break;
      case "set-options":
        requireController().setLogLevel(message.logLevel);
        requireController().setLookaheadCount(message.lookaheadCount);
        break;
      case "set-playback":
        followClock(message.clock);
        loop = message.loop;
        playbackRevision = message.revision;
        range = message.range;
        if (durationFrames > 0) {
          runner.setRange(applyPlaybackRange(requireController()), message.clock);
        }
        break;
    }
  }

  function requireController(): LumenPreviewController {
    if (!controller) {
      throw new Error("the preview worker has not started");
    }
    return controller;
  }

  function requireLifecycle(): MediaLifecycle<PreviewMediaInput> {
    if (!lifecycle) {
      throw new Error("the preview worker has not started");
    }
    return lifecycle;
  }

  /** Each kind's way into the renderer. Audio is decoded by the page, which plays it. */
  function mediaLoader(target: LumenPreviewController): MediaLoader<PreviewMediaInput> {
    return {
      load: async ({ kind, source }, { source: input, ...video }, signal) => {
        switch (kind) {
          case "image":
            return target.registerImageSource(source, input);
          case "video":
            return target.registerVideoSource(source, input, video);
          case "model":
            return target.registerModelSource(source, input);
          case "environment":
            return target.registerEnvironmentSource(source, input);
          case "font": {
            const bytes = await readFont(source, input, signal);
            if (!signal.aborted) {
              target.registerFontFamily(source, bytes);
            }
            return;
          }
          case "audio":
            return requestAudio(source, input, signal);
        }
      },
      release: ({ kind, source }) => {
        switch (kind) {
          case "image":
            return target.removeImageSource(source);
          case "video":
            return target.removeVideoSource(source);
          case "model":
            return target.removeModelSource(source);
          case "environment":
            return target.removeEnvironmentSource(source);
          case "font":
            return target.removeFontFamily(source);
          case "audio":
            return scope.postMessage({ source, type: "audio-source-released" });
        }
      },
      same: samePreviewMediaInput,
    };
  }

  function requestAudio(source: string, input: MediaSourceInput, signal: AbortSignal) {
    const requestId = ++audioRequestId;
    return new Promise<void>((resolve, reject) => {
      const settle = (failure: MediaFailure | null) => {
        audioRequests.delete(requestId);
        if (failure) {
          reject(failedMedia(failure));
        } else {
          resolve();
        }
      };
      audioRequests.set(requestId, settle);
      signal.addEventListener("abort", () => settle(null), { once: true });
      scope.postMessage({ input, requestId, source, type: "audio-source" });
    });
  }

  function postMedia(): void {
    scope.postMessage({ loadId, media: requireLifecycle().entries(), type: "media" });
  }

  async function start(
    message: Extract<LumenPreviewWorkerCommand, { type: "start" }>,
  ): Promise<void> {
    const { gpu } = navigator as WorkerNavigator;
    if (!gpu || !(await gpu.requestAdapter().catch(() => null))) {
      scope.postMessage({
        code: "webgpu_unavailable",
        message:
          "Lumen preview needs WebGPU, and the preview worker couldn't get a GPU adapter. Check that this browser has WebGPU enabled for this device.",
        type: "startup-failed",
      });
      return;
    }

    try {
      await initPreview({ module_or_path: message.wasmUrl });
      const { LumenPreviewController: PreviewController } =
        createLumenPreviewRuntime(previewBindings);
      controller = new PreviewController();
      lifecycle = new MediaLifecycle(mediaLoader(controller), postMedia);
      controller.setLogLevel(message.logLevel);
      controller.setLookaheadCount(message.lookaheadCount);
    } catch (error) {
      scope.postMessage({
        code: "worker_failed",
        message: `The Lumen preview worker couldn't load its wasm from ${message.wasmUrl}: ${describeError(error)}. ${assetHint(message.wasmUrl)}`,
        type: "startup-failed",
      });
      return;
    }
    scope.postMessage({ type: "ready" });
  }

  async function load(
    message: Extract<LumenPreviewWorkerCommand, { type: "load" }>,
  ): Promise<void> {
    const activeController = requireController();
    const activeLifecycle = requireLifecycle();
    const isCurrent = () => message.loadId === loadId;
    loadId = message.loadId;
    runner.unload();
    followClock(message.clock);
    lastRenderMs = 0;
    playbackFps.reset();
    replaceEntries(fonts, message.keepFonts, message.fonts);
    replaceEntries(media, message.keepMedia, message.media);

    try {
      // A composition the engine rejects fails before any media is fetched. It becomes the one
      // renders draw only once everything but its audio is in, as a render still in flight draws
      // whichever is loaded.
      const inventory = previewBindings.mediaDependencies(message.compositionJson);
      activeLifecycle.sync(previewDependencies(inventory, media, fonts));
      const failure = await activeLifecycle.settled(({ kind }) => kind !== "audio");
      if (!isCurrent()) {
        return;
      }
      if (failure) {
        throw failedMedia(failure);
      }
      activeController.loadComposition(message.compositionJson);
      if (canvas) {
        canvas.width = activeController.width() || 1;
        canvas.height = activeController.height() || 1;
      }
      scope.postMessage({
        audioTimelineJson: activeController.audioTimeline() ?? null,
        fps: activeController.fps(),
        height: activeController.height(),
        loadId,
        totalFrames: activeController.durationFrames(),
        type: "loaded",
        width: activeController.width(),
      });
      durationFrames = activeController.durationFrames();
      runner.load(
        durationFrames,
        activeController.fps(),
        clock,
        applyPlaybackRange(activeController),
      );
    } catch (error) {
      if (isCurrent()) {
        scope.postMessage({
          diagnostics: errorDiagnostics(error),
          loadId,
          media: errorMediaFailure(error),
          message: describeError(error),
          type: "load-failed",
        });
      }
    }
  }

  function applyPlaybackRange(activeController: LumenPreviewController) {
    const resolved = resolvePlaybackRange(range, loop, durationFrames);
    activeController.setPlaybackRange(resolved);
    return resolved;
  }

  function followClock(next: PreviewClock): void {
    if (next.rate > 0 && clock.rate <= 0) {
      playbackFps.reset();
    }
    clock = next;
  }
}

function replaceEntries<T>(
  current: Map<string, T>,
  keep: readonly string[],
  changed: readonly (readonly [string, T])[],
): void {
  const kept = new Set(keep);
  for (const key of current.keys()) {
    if (!kept.has(key)) {
      current.delete(key);
    }
  }
  for (const [key, value] of changed) {
    current.set(key, value);
  }
}

async function readFont(
  family: string,
  source: MediaSourceInput,
  signal: AbortSignal,
): Promise<ArrayBuffer> {
  try {
    const blob = await sourceInputToBlob(source, signal);
    return await blob.arrayBuffer();
  } catch (error) {
    throw toMediaError("invalid_source", family, `font "${family}" could not be loaded`, error, {
      wanted: ["font"],
    });
  }
}

/** An error carrying `failure`, as the wasm bindings throw for a source the engine can't use. */
function failedMedia(failure: MediaFailure): Error {
  return Object.assign(new Error(`${failure.source}: ${failure.details}`), { media: failure });
}
