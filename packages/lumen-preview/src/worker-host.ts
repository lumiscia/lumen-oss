import type { LumenDiagnostic } from "@lumiscia/lumen-types";
import { epochNow, type PreviewClock } from "./clock.js";
import {
  createLumenPreviewRuntime,
  type LumenLogLevel,
  type LumenPreviewBindings,
  type LumenPreviewController,
  type MediaRegistration,
  type MediaSourceInput,
} from "./index.js";
import { sameMediaInput, sourceInputToBlob } from "./media/source.js";
import type { LumenPreviewErrorCode } from "./preview.js";
import { resolvePlaybackRange, type LumenPreviewRange } from "./playback-range.js";
import { PreviewRunner } from "./runner.js";
import { assetHint } from "./asset-hint.js";
import { describeError, errorDiagnostics } from "./session/errors.js";
import { PlaybackFpsMeter } from "./session/stats.js";

type InitPreview = (options: { module_or_path: string }) => Promise<object>;

export interface LumenPreviewWorkerHostOptions {
  initPreview: InitPreview;
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
      media: readonly MediaRegistration[];
      type: "load";
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
      message: string;
      type: "render-failed";
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
  const fonts = new Map<string, MediaSourceInput>();
  const loadedFonts = new Map<string, MediaSourceInput>();
  const media = new Map<string, MediaRegistration>();
  const playbackFps = new PlaybackFpsMeter();
  // The newest clock the page sent, which a load starts on.
  let clock: PreviewClock = { atEpochMs: 0, originMs: 0, rate: 0 };
  let canvas: OffscreenCanvas | null = null;
  let controller: LumenPreviewController | null = null;
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
        scope.postMessage({ loadId, message: describeError(error), type: "render-failed" });
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
        loadedFonts.clear();
        requireController().clear();
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
    const isCurrent = () => message.loadId === loadId;
    loadId = message.loadId;
    runner.unload();
    followClock(message.clock);
    lastRenderMs = 0;
    playbackFps.reset();
    replaceEntries(fonts, message.keepFonts, message.fonts);
    replaceEntries(
      media,
      message.keepMedia,
      message.media.map((registration) => [registration.id, registration]),
    );

    try {
      // A composition the engine rejects fails before any media is fetched. It becomes the one
      // renders draw only once its media is in, as a render still in flight draws whichever is.
      activeController.validateComposition(message.compositionJson);
      await syncFonts(activeController, isCurrent);
      await activeController.syncMediaSources([...media.values()]);
      if (!isCurrent()) {
        return;
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

  async function syncFonts(
    activeController: LumenPreviewController,
    isCurrent: () => boolean,
  ): Promise<void> {
    for (const family of loadedFonts.keys()) {
      if (!fonts.has(family)) {
        loadedFonts.delete(family);
        activeController.removeFontFamily(family);
      }
    }
    await Promise.all(
      [...fonts].map(async ([family, source]) => {
        const loaded = loadedFonts.get(family);
        if (loaded && sameMediaInput(loaded, source)) {
          return;
        }
        const bytes = await readFont(family, source);
        if (isCurrent()) {
          activeController.registerFontFamily(family, bytes);
          loadedFonts.set(family, source);
        }
      }),
    );
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

async function readFont(family: string, source: MediaSourceInput): Promise<ArrayBuffer> {
  try {
    const blob = await sourceInputToBlob(source);
    return await blob.arrayBuffer();
  } catch (error) {
    throw new Error(`font "${family}" could not be loaded: ${describeError(error)}`, {
      cause: error,
    });
  }
}
