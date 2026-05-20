import {
  createLumenPreviewRuntime,
  type LumenLogLevel,
  type LumenPreviewBindings,
  type LumenPreviewController,
  type MediaRegistration,
} from "./index.js";
import type { LumenPreviewPatch } from "./preview.js";
import { describeError } from "./session/errors.js";
import { EMPTY_PREVIEW_STATS, PlaybackFpsMeter, type LumenPreviewStats } from "./session/stats.js";

type PreviewWasmInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

type InitPreview = (options: {
  module_or_path: PreviewWasmInput | Promise<PreviewWasmInput>;
}) => Promise<object>;

export interface LumenPreviewWorkerHostOptions {
  initPreview: InitPreview;
  previewBindings: LumenPreviewBindings;
  previewWasmUrl: PreviewWasmInput | Promise<PreviewWasmInput>;
}

export type LumenPreviewWorkerCommand =
  | {
      canvas: OffscreenCanvas;
      compositionJson: string | null;
      logLevel: LumenLogLevel;
      mediaSources: readonly MediaRegistration[];
      type: "initialize";
    }
  | {
      compositionJson: string | null;
      mediaSources: readonly MediaRegistration[];
      type: "set-composition";
    }
  | {
      logLevel: LumenLogLevel;
      type: "set-log-level";
    }
  | {
      fromMs: number;
      type: "play";
    }
  | {
      type: "pause";
    }
  | {
      frame: number;
      type: "seek";
    }
  | {
      type: "dispose";
    };

export type LumenPreviewWorkerEvent =
  | {
      patch: LumenPreviewPatch;
      type: "state";
    }
  | {
      stats: LumenPreviewStats;
      type: "stats";
    }
  | {
      message: string;
      scope: string;
      type: "error";
    };

type PreviewWorkerScope = typeof globalThis & {
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<LumenPreviewWorkerCommand>) => void,
  ): void;
  postMessage(message: LumenPreviewWorkerEvent): void;
};

export function createLumenPreviewWorkerHost({
  initPreview,
  previewBindings,
  previewWasmUrl,
}: LumenPreviewWorkerHostOptions): void {
  const scope = globalThis as PreviewWorkerScope;
  let canvas: OffscreenCanvas | null = null;
  let compositionJson: string | null = null;
  let controller: LumenPreviewController | null = null;
  let fps = 0;
  let frameMs = 1_000;
  let lastRenderMs = 0;
  let loadGeneration = 0;
  let mediaSources: readonly MediaRegistration[] = [];
  let playbackFps = new PlaybackFpsMeter();
  let playing = false;
  let queuedRender = false;
  let renderInFlight = false;
  let timerId: ReturnType<typeof setTimeout> | null = null;

  scope.addEventListener("message", (event) => {
    void handleMessage(event.data).catch((error) => reportError("worker message", error));
  });

  async function handleMessage(message: LumenPreviewWorkerCommand): Promise<void> {
    switch (message.type) {
      case "initialize":
        await initialize(message);
        break;
      case "set-log-level":
        controller?.setLogLevel(message.logLevel);
        break;
      case "set-composition":
        compositionJson = message.compositionJson;
        mediaSources = message.mediaSources;
        await loadComposition();
        break;
      case "play":
        play(message.fromMs);
        break;
      case "pause":
        pause();
        break;
      case "seek":
        seek(message.frame);
        break;
      case "dispose":
        dispose();
        break;
    }
  }

  async function initialize(
    message: Extract<LumenPreviewWorkerCommand, { type: "initialize" }>,
  ): Promise<void> {
    dispose();
    canvas = message.canvas;
    compositionJson = message.compositionJson;
    mediaSources = message.mediaSources;

    await initPreview({ module_or_path: previewWasmUrl });
    const { LumenPreviewController: PreviewController } =
      createLumenPreviewRuntime(previewBindings);
    controller = new PreviewController();
    controller.setLogLevel(message.logLevel);

    await loadComposition();
  }

  async function loadComposition(): Promise<void> {
    const activeController = controller;
    if (!activeController || !canvas) {
      return;
    }

    const generation = ++loadGeneration;
    playing = false;
    stopLoop();
    activeController.pause();

    if (!compositionJson) {
      activeController.clear();
      setFps(0);
      lastRenderMs = 0;
      postState({
        frame: 0,
        totalFrames: 0,
        width: 0,
        height: 0,
        isLoaded: false,
      });
      playbackFps.reset();
      postStats(EMPTY_PREVIEW_STATS);
      return;
    }

    try {
      await activeController.syncMediaSources(mediaSources);
      if (generation !== loadGeneration) {
        return;
      }
      activeController.loadComposition(compositionJson);
      setFps(activeController.fps());
      canvas.width = activeController.width() || 1;
      canvas.height = activeController.height() || 1;
      postState({
        error: null,
        frame: activeController.currentFrame(),
        height: activeController.height(),
        isLoaded: true,
        isPlaying: false,
        totalFrames: activeController.durationFrames(),
        width: activeController.width(),
      });
      postStats(statsFromController(activeController, 0));
      queueRender();
    } catch (error) {
      setFps(0);
      lastRenderMs = 0;
      playbackFps.reset();
      postState({
        error: describeError(error),
        isLoaded: false,
      });
      postStats(EMPTY_PREVIEW_STATS);
    }
  }

  function play(fromMs: number): void {
    const activeController = controller;
    if (!activeController) {
      return;
    }
    playing = true;
    playbackFps.reset();
    activeController.setFrame(activeController.targetFrameForTimeMs(fromMs));
    activeController.play();
    postState({ isPlaying: true });
    startLoop();
  }

  function pause(): void {
    playing = false;
    controller?.pause();
    stopLoop();
    postState({ frame: controller?.currentFrame() ?? 0, isPlaying: false });
    postStats(controller ? statsFromController(controller) : EMPTY_PREVIEW_STATS);
  }

  function seek(frame: number): void {
    if (!controller) {
      return;
    }
    controller.setFrame(frame);
    postState({ frame: controller.currentFrame() });
    queueRender();
  }

  function startLoop(): void {
    if (timerId) {
      return;
    }

    const tick = () => {
      timerId = null;
      if (!playing || !controller || !canvas) {
        return;
      }
      queueRender();
      timerId = setTimeout(tick, frameMs);
    };
    timerId = setTimeout(tick, 0);
  }

  function stopLoop(): void {
    if (timerId) {
      clearTimeout(timerId);
      timerId = null;
    }
  }

  function queueRender(): void {
    queuedRender = true;
    if (!renderInFlight) {
      void drainRenderQueue();
    }
  }

  async function drainRenderQueue(): Promise<void> {
    renderInFlight = true;
    try {
      while (queuedRender) {
        queuedRender = false;
        await renderOnce();
      }
    } finally {
      renderInFlight = false;
      if (queuedRender) {
        void drainRenderQueue();
      }
    }
  }

  async function renderOnce(): Promise<void> {
    const activeController = controller;
    if (!activeController || !canvas || activeController.durationFrames() <= 0) {
      return;
    }

    try {
      const startedAt = performance.now();
      let changed = true;
      if (playing) {
        changed = await activeController.tickAsync(performance.now(), canvas);
      } else {
        await activeController.renderNowAsync(canvas);
      }

      if (changed || !playing) {
        lastRenderMs = performance.now() - startedAt;
        postState({
          error: null,
          frame: activeController.currentFrame(),
          isPlaying: playing,
        });
        postStats(statsFromController(activeController));
      }
    } catch (error) {
      reportError("render", error);
    }
  }

  function dispose(): void {
    stopLoop();
    playing = false;
    controller?.clear();
    controller = null;
    canvas = null;
    renderInFlight = false;
    queuedRender = false;
    playbackFps.reset();
    lastRenderMs = 0;
  }

  function setFps(nextFps: number): void {
    fps = nextFps || 0;
    frameMs = 1_000 / Math.max(fps, 1);
  }

  function postState(patch: LumenPreviewPatch): void {
    scope.postMessage({ patch, type: "state" });
  }

  function postStats(stats: LumenPreviewStats): void {
    scope.postMessage({ stats, type: "stats" });
  }

  function statsFromController(
    activeController: LumenPreviewController,
    renderMs = lastRenderMs,
  ): LumenPreviewStats {
    const frame = activeController.currentFrame();
    return {
      frame,
      timelineFps: activeController.fps(),
      targetFrameDurationMs: activeController.targetFrameDurationMs(),
      renderMs,
      actualFps: playbackFps.sample(frame, playing),
    };
  }

  function reportError(scopeName: string, error: unknown): void {
    scope.postMessage({
      message: describeError(error),
      scope: scopeName,
      type: "error",
    });
  }
}
