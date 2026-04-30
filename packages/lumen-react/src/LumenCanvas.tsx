import { useEffect, useRef } from "react";

import type { CSSProperties } from "react";
import type {
  AudioEngineTimeline,
  AudioSourceRegistration,
  LumenAudioEngine,
  LumenPreviewController,
} from "lumen-wasm";

import type { LumenPreviewContext } from "./preview.ts";

export interface LumenCanvasProps {
  preview: LumenPreviewContext;
  audioSources?: AudioSourceRegistration[];
  audioTimeline?: AudioEngineTimeline | null;
  compositionJson?: string | null;
  fps?: number;
  className?: string;
  style?: CSSProperties;
}

export function LumenCanvas({
  preview,
  audioSources = [],
  audioTimeline = null,
  compositionJson = null,
  fps = 30,
  className,
  style,
}: LumenCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const audioEngineRef = useRef<LumenAudioEngine | null>(null);
  const controllerRef = useRef<LumenPreviewController | null>(null);
  const queuedRenderRef = useRef<(() => Promise<void>) | null>(null);
  const renderInFlightRef = useRef(false);
  const prefetchGenerationRef = useRef(0);
  const prefetchesRef = useRef(new Map<number, Promise<void>>());
  const animationFrameRef = useRef(0);

  const PREFETCH_AHEAD_FRAMES = 6;

  function getCtx2d(): CanvasRenderingContext2D | null {
    return canvasRef.current?.getContext("2d") ?? null;
  }

  function describeError(error: unknown): string {
    if (error instanceof Error) {
      return error.stack ?? `${error.name}: ${error.message}`;
    }

    return String(error);
  }

  function reportError(scope: string, error: unknown): void {
    console.error(`[LumenCanvas] ${scope}`, error);
    preview.update({ error: describeError(error) });
  }

  function normalizeFrame(frame: number): number {
    const totalFrames = preview.getSnapshot().totalFrames;
    if (totalFrames <= 0) {
      return frame;
    }

    return ((frame % totalFrames) + totalFrames) % totalFrames;
  }

  async function preloadFrame(controller: LumenPreviewController, frame: number): Promise<void> {
    const totalFrames = preview.getSnapshot().totalFrames;
    const normalizedFrame = normalizeFrame(frame);
    if (normalizedFrame < 0 || (totalFrames > 0 && normalizedFrame >= totalFrames)) {
      return;
    }

    const existing = prefetchesRef.current.get(normalizedFrame);
    if (existing) {
      await existing;
      return;
    }

    const generation = prefetchGenerationRef.current;
    const pending = controller.preloadFrame(normalizedFrame).finally(() => {
      if (prefetchesRef.current.get(normalizedFrame) === pending) {
        prefetchesRef.current.delete(normalizedFrame);
      }
    });
    prefetchesRef.current.set(normalizedFrame, pending);

    try {
      await pending;
    } catch (error) {
      if (generation !== prefetchGenerationRef.current) {
        return;
      }

      throw error;
    }
  }

  function resetPrefetch(): void {
    prefetchGenerationRef.current += 1;
    prefetchesRef.current.clear();
  }

  function prefetchAhead(controller: LumenPreviewController, frame: number): void {
    for (let offset = 1; offset <= PREFETCH_AHEAD_FRAMES; offset += 1) {
      void preloadFrame(controller, frame + offset).catch(() => {
        // The render path surfaces persistent errors.
      });
    }
  }

  async function preloadPlaybackWindow(
    controller: LumenPreviewController,
    frame: number,
  ): Promise<void> {
    await preloadFrame(controller, frame);
    await preloadFrame(controller, frame + 1);
    prefetchAhead(controller, frame);
  }

  function queueRender(operation: () => Promise<void>): void {
    queuedRenderRef.current = operation;
    if (renderInFlightRef.current) {
      return;
    }

    void drainRenderQueue();
  }

  async function drainRenderQueue(): Promise<void> {
    renderInFlightRef.current = true;

    try {
      while (queuedRenderRef.current) {
        const next = queuedRenderRef.current;
        queuedRenderRef.current = null;
        await next();
      }
    } finally {
      renderInFlightRef.current = false;
      if (queuedRenderRef.current) {
        void drainRenderQueue();
      }
    }
  }

  async function renderOnce(controller: LumenPreviewController): Promise<void> {
    const ctx = getCtx2d();
    if (!ctx) {
      return;
    }

    try {
      await preloadPlaybackWindow(controller, controller.currentFrame());
      const startedAt = performance.now();
      controller.renderNow(ctx);
      preview.update({
        frame: controller.currentFrame(),
        renderMs: performance.now() - startedAt,
        isPlaying: controller.isPlaying(),
        error: null,
      });
    } catch (error) {
      reportError("renderOnce", error);
    }
  }

  function handleLoadComposition(
    controller: LumenPreviewController,
    json: string,
    nextFps: number,
  ): void {
    try {
      controller.loadComposition(json, nextFps);
      preview.update({
        totalFrames: controller.durationFrames(),
        width: controller.width(),
        height: controller.height(),
        error: null,
      });
    } catch (error) {
      reportError("loadComposition", error);
    }
  }

  function startLoop(controller: LumenPreviewController): void {
    cancelAnimationFrame(animationFrameRef.current);
    animationFrameRef.current = 0;

    const loop = (now: number): void => {
      animationFrameRef.current = 0;
      const ctx = getCtx2d();

      if (ctx) {
        queueRender(async () => {
          const audioEngine = audioEngineRef.current;
          if (audioEngine && audioTimeline && preview.getSnapshot().isPlaying) {
            const targetFrame = controller.targetFrameForTimeMs(audioEngine.currentTimeMs());
            try {
              await preloadPlaybackWindow(controller, targetFrame);
              controller.setFrame(targetFrame);
              const startedAt = performance.now();
              controller.renderNow(ctx);
              preview.update({
                frame: targetFrame,
                renderMs: performance.now() - startedAt,
                isPlaying: controller.isPlaying(),
                error: null,
              });
              prefetchAhead(controller, targetFrame);
            } catch (error) {
              reportError("audio-clock animation tick", error);
            }
            return;
          }

          try {
            const currentFrame = controller.currentFrame();
            await preloadPlaybackWindow(controller, currentFrame);
          } catch {
            // Keep the loop alive while media settles.
          }

          try {
            const startedAt = performance.now();
            const changed = controller.tick(now, ctx);
            if (changed) {
              const currentFrame = controller.currentFrame();
              preview.update({
                frame: currentFrame,
                renderMs: performance.now() - startedAt,
                isPlaying: controller.isPlaying(),
                error: null,
              });
              prefetchAhead(controller, currentFrame);
            }
          } catch (error) {
            reportError("animation tick", error);
          }
        });
      }

      animationFrameRef.current = requestAnimationFrame(loop);
    };

    animationFrameRef.current = requestAnimationFrame(loop);
  }

  useEffect(() => {
    let cancelled = false;

    void import("lumen-wasm")
      .then(({ LumenAudioEngine: AudioEngine, LumenPreviewController: PreviewController }) => {
        if (cancelled) {
          return;
        }

        const controller = new PreviewController();
        const audioEngine = new AudioEngine();
        audioEngineRef.current = audioEngine;
        controllerRef.current = controller;
        preview._attach(
          controller,
          (frame) => {
            queueRender(async () => {
              try {
                resetPrefetch();
                controller.setFrame(frame);
                await renderOnce(controller);
              } catch (error) {
                reportError("seek render", error);
              }
            });
          },
          {
            pause: () => audioEngine.pause(),
            play: () => {
              const frame = controller.currentFrame();
              audioEngine.play((frame / Math.max(fps, 1)) * 1000);
            },
            seek: (frame) => audioEngine.seekMs((frame / Math.max(fps, 1)) * 1000),
          },
        );
        startLoop(controller);
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          reportError("import lumen-wasm", error);
        }
      });

    return () => {
      cancelled = true;
      cancelAnimationFrame(animationFrameRef.current);
      animationFrameRef.current = 0;
      resetPrefetch();
      audioEngineRef.current?.dispose();
      audioEngineRef.current = null;
      controllerRef.current?.clear();
      controllerRef.current = null;
      preview._detach();
    };
  }, [preview]);

  useEffect(() => {
    const audioEngine = audioEngineRef.current;
    if (!audioEngine) {
      return;
    }

    audioEngine.setAudioTimeline(audioTimeline);
    void audioEngine.syncAudioSources(audioSources).catch((error: unknown) => {
      reportError("sync audio sources", error);
    });
  }, [audioSources, audioTimeline]);

  useEffect(() => {
    const controller = controllerRef.current;
    if (!controller || !compositionJson) {
      return;
    }

    resetPrefetch();
    handleLoadComposition(controller, compositionJson, fps);
    queueRender(() => renderOnce(controller));
  }, [compositionJson, fps, preview]);

  const snapshot = preview.getSnapshot();

  return (
    <canvas
      ref={canvasRef}
      width={snapshot.width || 1}
      height={snapshot.height || 1}
      className={className}
      style={style}
    />
  );
}
