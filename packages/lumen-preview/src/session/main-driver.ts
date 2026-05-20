import { LumenAudioEngine, audioTimelineFromCompositionJson } from "../audio-engine.js";
import { createLumenPreviewRuntime, resolveLumenPreviewBindings } from "../index.js";
import type { LumenPreviewController } from "../index.js";
import { LumenRenderQueue } from "./render-queue.js";
import { EMPTY_PREVIEW_STATS, PlaybackFpsMeter } from "./stats.js";
import type {
  LumenPreviewDriverHost,
  LumenPreviewRuntimeDriver,
  LumenPreviewSessionInputs,
  LumenPreviewSessionOptions,
} from "./types.js";

export class MainPreviewDriver implements LumenPreviewRuntimeDriver {
  #audio: LumenAudioEngine;
  #canvas: HTMLCanvasElement | null = null;
  #controller: LumenPreviewController | null = null;
  #inputs: LumenPreviewSessionInputs;
  #isLoaded = false;
  #host: LumenPreviewDriverHost;
  #lastRenderMs = 0;
  #loadGeneration = 0;
  #playbackFps = new PlaybackFpsMeter();
  #queue = new LumenRenderQueue();
  #rafId = 0;
  #disposed = false;

  constructor(
    host: LumenPreviewDriverHost,
    inputs: LumenPreviewSessionInputs,
    options: LumenPreviewSessionOptions,
  ) {
    this.#host = host;
    this.#inputs = inputs;
    this.#audio = new LumenAudioEngine(options.audio);
  }

  async attach(canvas: HTMLCanvasElement): Promise<void> {
    this.#canvas = canvas;
    const bindings = await resolveLumenPreviewBindings(this.#inputs.bindings);
    if (this.#disposed) {
      return;
    }

    const { LumenPreviewController: PreviewController } = createLumenPreviewRuntime(bindings);
    const controller = new PreviewController();
    controller.setLogLevel(this.#inputs.logLevel);
    this.#controller = controller;
    this.#syncAudio();

    this.#host.attachController(controller, (frame) => this.#seek(frame), {
      pause: () => {
        this.#audio.pause();
        this.#reportStats({
          frame: controller.currentFrame(),
          isPlaying: false,
        });
      },
      play: () => {
        this.#playbackFps.reset();
        this.#audio.play(this.#frameToMs(controller.currentFrame()));
      },
      seek: (frame) => this.#audio.seekMs(this.#frameToMs(frame)),
    });

    await this.#loadComposition();
    this.#startLoop();
  }

  update(inputs: LumenPreviewSessionInputs): void {
    const previous = this.#inputs;
    this.#inputs = inputs;

    this.#controller?.setLogLevel(inputs.logLevel);
    this.#syncAudio();

    if (
      previous.compositionJson !== inputs.compositionJson ||
      previous.mediaSources !== inputs.mediaSources
    ) {
      void this.#loadComposition();
    }
  }

  dispose(): void {
    this.#disposed = true;
    this.#stopLoop();
    this.#queue.clear();
    this.#audio.dispose();
    this.#controller?.clear();
    this.#controller = null;
    this.#canvas = null;
    this.#isLoaded = false;
    this.#host.detachController();
  }

  async #loadComposition(): Promise<void> {
    const controller = this.#controller;
    if (!controller) {
      return;
    }

    const generation = ++this.#loadGeneration;
    this.#isLoaded = false;

    if (!this.#inputs.compositionJson) {
      controller.clear();
      this.#host.updateState({
        frame: 0,
        totalFrames: 0,
        width: 0,
        height: 0,
        isLoaded: false,
      });
      this.#playbackFps.reset();
      this.#lastRenderMs = 0;
      this.#host.reportStats(EMPTY_PREVIEW_STATS);
      return;
    }

    try {
      await controller.syncMediaSources(this.#inputs.mediaSources);
      if (generation !== this.#loadGeneration || this.#disposed) {
        return;
      }
      controller.loadComposition(this.#inputs.compositionJson);
      this.#isLoaded = true;
      this.#host.updateState({
        totalFrames: controller.durationFrames(),
        width: controller.width(),
        height: controller.height(),
        isLoaded: true,
        error: null,
      });
      this.#reportStats({ frame: 0, renderMs: 0, isPlaying: false });
      this.#queue.enqueue(() => this.#renderOnce());
    } catch (error) {
      if (generation === this.#loadGeneration) {
        this.#isLoaded = false;
        this.#playbackFps.reset();
        this.#lastRenderMs = 0;
        this.#host.updateState({ isLoaded: false });
        this.#host.reportStats(EMPTY_PREVIEW_STATS);
        this.#host.reportError("load composition", error);
      }
    }
  }

  #startLoop(): void {
    this.#stopLoop();

    const loop = (now: number): void => {
      this.#rafId = 0;
      if (this.#canvas && this.#isLoaded) {
        const isPlaying = this.#controller?.isPlaying() ?? false;
        this.#queue.enqueue(async () => {
          if (isPlaying || this.#hostIsPlaying()) {
            await this.#renderPlaybackFrame(now);
          }
        });
      }
      this.#rafId = requestAnimationFrame(loop);
    };

    this.#rafId = requestAnimationFrame(loop);
  }

  #stopLoop(): void {
    cancelAnimationFrame(this.#rafId);
    this.#rafId = 0;
  }

  async #renderPlaybackFrame(now: number): Promise<void> {
    const controller = this.#controller;
    const canvas = this.#canvas;
    if (!controller || !canvas || !this.#isLoaded) {
      return;
    }

    if (this.#audio.isPlaying() && this.#audioTimeline()) {
      const targetFrame = controller.targetFrameForTimeMs(this.#audio.currentTimeMs());
      try {
        controller.setFrame(targetFrame);
        const startedAt = performance.now();
        await controller.renderNowAsync(canvas);
        const renderMs = performance.now() - startedAt;
        const isPlaying = controller.isPlaying();
        this.#host.updateState({
          frame: targetFrame,
          isPlaying,
          error: null,
        });
        this.#reportStats({ frame: targetFrame, renderMs, isPlaying });
      } catch (error) {
        this.#host.reportError("audio-clock animation tick", error);
      }
      return;
    }

    try {
      const startedAt = performance.now();
      const changed = await controller.tickAsync(now, canvas);
      if (changed) {
        const frame = controller.currentFrame();
        const renderMs = performance.now() - startedAt;
        const isPlaying = controller.isPlaying();
        this.#host.updateState({
          frame,
          isPlaying,
          error: null,
        });
        this.#reportStats({ frame, renderMs, isPlaying });
      }
    } catch (error) {
      this.#host.reportError("animation tick", error);
    }
  }

  async #renderOnce(): Promise<void> {
    const controller = this.#controller;
    const canvas = this.#canvas;
    if (!controller || !canvas || !this.#isLoaded) {
      return;
    }

    try {
      const startedAt = performance.now();
      await controller.renderNowAsync(canvas);
      const frame = controller.currentFrame();
      const renderMs = performance.now() - startedAt;
      const isPlaying = controller.isPlaying();
      this.#host.updateState({
        frame,
        isPlaying,
        error: null,
      });
      this.#reportStats({ frame, renderMs, isPlaying });
    } catch (error) {
      this.#host.reportError("render once", error);
    }
  }

  #seek(frame: number): void {
    this.#queue.enqueue(async () => {
      try {
        this.#controller?.setFrame(frame);
        await this.#renderOnce();
      } catch (error) {
        this.#host.reportError("seek render", error);
      }
    });
  }

  #syncAudio(): void {
    const timeline = this.#audioTimeline();
    this.#audio.setAudioTimeline(timeline);
    void this.#audio.syncAudioSources(this.#inputs.audioSources).catch((error: unknown) => {
      this.#host.reportError("sync audio sources", error);
    });
  }

  #audioTimeline() {
    return (
      this.#inputs.audioTimeline ?? audioTimelineFromCompositionJson(this.#inputs.compositionJson)
    );
  }

  #frameToMs(frame: number): number {
    return (frame / Math.max(this.#inputs.fps, 1)) * 1_000;
  }

  #reportStats({
    frame,
    renderMs,
    isPlaying,
  }: {
    frame: number;
    renderMs?: number;
    isPlaying: boolean;
  }): void {
    const controller = this.#controller;
    if (!controller) {
      this.#playbackFps.reset();
      this.#lastRenderMs = 0;
      this.#host.reportStats(EMPTY_PREVIEW_STATS);
      return;
    }

    if (renderMs !== undefined) {
      this.#lastRenderMs = renderMs;
    }

    this.#host.reportStats({
      frame,
      timelineFps: controller.fps(),
      targetFrameDurationMs: controller.targetFrameDurationMs(),
      renderMs: this.#lastRenderMs,
      actualFps: this.#playbackFps.sample(frame, isPlaying),
    });
  }

  #hostIsPlaying(): boolean {
    return this.#controller?.isPlaying() ?? false;
  }
}
