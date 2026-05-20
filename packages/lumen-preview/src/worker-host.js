import { createLumenPreviewRuntime } from "./index.js";

export function createLumenPreviewWorkerHost({ initPreview, previewBindings, previewWasmUrl }) {
  let canvas = null;
  let controller = null;
  let playing = false;
  let rafId = 0;
  let frameMs = 1_000 / 30;
  let fps = 30;
  let compositionJson = null;
  let mediaSources = [];
  let loadGeneration = 0;
  let renderInFlight = false;
  let queuedRender = false;

  globalThis.onmessage = (event) => {
    void handleMessage(event.data).catch((error) => reportError("worker message", error));
  };

  async function handleMessage(message) {
    switch (message.type) {
      case "initialize":
        await initialize(message);
        break;
      case "set-log-level":
        controller?.setLogLevel(message.logLevel);
        break;
      case "set-composition":
        compositionJson = message.compositionJson;
        fps = message.fps || 30;
        frameMs = 1_000 / Math.max(fps, 1);
        mediaSources = message.mediaSources || [];
        await loadComposition();
        break;
      case "set-media-sources":
        mediaSources = message.mediaSources || [];
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
      case "render":
        queueRender();
        break;
      case "dispose":
        dispose();
        break;
    }
  }

  async function initialize(message) {
    dispose();
    canvas = message.canvas;
    fps = message.fps || 30;
    frameMs = 1_000 / Math.max(fps, 1);
    compositionJson = message.compositionJson;
    mediaSources = message.mediaSources || [];

    await initPreview({ module_or_path: previewWasmUrl });
    const { LumenPreviewController } = createLumenPreviewRuntime(previewBindings);
    controller = new LumenPreviewController();
    controller.setLogLevel(message.logLevel || "off");

    await loadComposition();
  }

  async function loadComposition() {
    const nextController = controller;
    if (!nextController || !canvas) {
      return;
    }

    const generation = ++loadGeneration;
    playing = false;
    stopLoop();
    nextController.pause();

    if (!compositionJson) {
      nextController.clear();
      postState({ isLoaded: false });
      return;
    }

    try {
      await nextController.syncMediaSources(mediaSources);
      if (generation !== loadGeneration) {
        return;
      }
      nextController.loadComposition(compositionJson, fps);
      canvas.width = nextController.width() || 1;
      canvas.height = nextController.height() || 1;
      postState({
        frame: nextController.currentFrame(),
        height: nextController.height(),
        isLoaded: true,
        isPlaying: false,
        totalFrames: nextController.durationFrames(),
        width: nextController.width(),
        error: null,
      });
      queueRender();
    } catch (error) {
      postState({ isLoaded: false, error: describeError(error) });
    }
  }

  function play(fromMs = 0) {
    const nextController = controller;
    if (!nextController) {
      return;
    }
    playing = true;
    nextController.setFrame(nextController.targetFrameForTimeMs(fromMs));
    nextController.play();
    postState({ isPlaying: true });
    startLoop();
  }

  function pause() {
    playing = false;
    controller?.pause();
    stopLoop();
    postState({ isPlaying: false, frame: controller?.currentFrame() ?? 0 });
  }

  function seek(frame) {
    if (!controller) {
      return;
    }
    controller.setFrame(frame);
    postState({ frame: controller.currentFrame() });
    queueRender();
  }

  function startLoop() {
    if (rafId) {
      return;
    }

    const tick = () => {
      rafId = 0;
      if (!playing || !controller || !canvas) {
        return;
      }
      queueRender();
      rafId = setTimeout(tick, frameMs);
    };
    rafId = setTimeout(tick, 0);
  }

  function stopLoop() {
    if (rafId) {
      clearTimeout(rafId);
      rafId = 0;
    }
  }

  function queueRender() {
    queuedRender = true;
    if (renderInFlight) {
      return;
    }
    void drainRenderQueue();
  }

  async function drainRenderQueue() {
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

  async function renderOnce() {
    const nextController = controller;
    if (!nextController || !canvas || nextController.durationFrames() <= 0) {
      return;
    }

    try {
      const startedAt = performance.now();
      let changed = true;
      if (playing) {
        changed = await nextController.tickAsync(performance.now(), canvas);
      } else {
        await nextController.renderNowAsync(canvas);
      }
      if (changed || !playing) {
        postState({
          frame: nextController.currentFrame(),
          renderMs: performance.now() - startedAt,
          isPlaying: playing,
          error: null,
        });
      }
    } catch (error) {
      reportError("render", error);
    }
  }

  function dispose() {
    stopLoop();
    playing = false;
    controller?.clear();
    controller = null;
    canvas = null;
    renderInFlight = false;
    queuedRender = false;
  }

  function postState(patch) {
    globalThis.postMessage({ type: "state", patch });
  }

  function reportError(scope, error) {
    globalThis.postMessage({
      type: "error",
      scope,
      message: describeError(error),
    });
  }

  function describeError(error) {
    if (error instanceof Error) {
      return error.stack || `${error.name}: ${error.message}`;
    }
    return String(error);
  }
}
