import { useEffect, useRef } from "react";

import type { CSSProperties } from "react";
import { LumenAudioEngine, createLumenPreviewRuntime } from "lumen-preview";
import type {
  AudioEngineTimeline,
  AudioSourceRegistration,
  LumenPreviewBindings,
  LumenPreviewController,
  MediaRegistration,
} from "lumen-preview";

import type { LumenPreviewContext } from "./preview.ts";

export interface LumenCanvasProps {
  preview: LumenPreviewContext;
  bindings: LumenPreviewBindings;
  audioSources?: AudioSourceRegistration[];
  audioTimeline?: AudioEngineTimeline | null;
  compositionJson?: string | null;
  fps?: number;
  mediaSources?: MediaRegistration[];
  className?: string;
  style?: CSSProperties;
}

export function LumenCanvas({
  preview,
  bindings,
  audioSources = [],
  audioTimeline = null,
  compositionJson = null,
  fps = 30,
  mediaSources = [],
  className,
  style,
}: LumenCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const audioEngineRef = useRef<LumenAudioEngine | null>(null);
  const controllerRef = useRef<LumenPreviewController | null>(null);
  const queuedRenderRef = useRef<(() => Promise<void>) | null>(null);
  const renderInFlightRef = useRef(false);
  const animationFrameRef = useRef(0);
  const compositionLoadedRef = useRef(false);

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

  function hasLoadedComposition(): boolean {
    return compositionLoadedRef.current;
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
    const canvas = canvasRef.current;
    if (!canvas) {
      return;
    }
    if (!hasLoadedComposition()) {
      return;
    }

    try {
      const startedAt = performance.now();
      await controller.renderNowAsync(canvas);
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
      compositionLoadedRef.current = true;
      preview.update({
        totalFrames: controller.durationFrames(),
        width: controller.width(),
        height: controller.height(),
        isLoaded: true,
        error: null,
      });
    } catch (error) {
      compositionLoadedRef.current = false;
      preview.update({ isLoaded: false });
      reportError("loadComposition", error);
    }
  }

  function startLoop(controller: LumenPreviewController): void {
    cancelAnimationFrame(animationFrameRef.current);
    animationFrameRef.current = 0;

    const loop = (now: number): void => {
      animationFrameRef.current = 0;
      const canvas = canvasRef.current;

      if (canvas) {
        const audioEngine = audioEngineRef.current;
        const snapshot = preview.getSnapshot();
        const shouldRenderTick =
          hasLoadedComposition() && (controller.isPlaying() || snapshot.isPlaying);

        if (shouldRenderTick) {
          queueRender(async () => {
            if (!hasLoadedComposition()) {
              return;
            }

            if (audioEngine && audioTimeline && preview.getSnapshot().isPlaying) {
              const targetFrame = controller.targetFrameForTimeMs(audioEngine.currentTimeMs());
              try {
                controller.setFrame(targetFrame);
                const startedAt = performance.now();
                await controller.renderNowAsync(canvas);
                preview.update({
                  frame: targetFrame,
                  renderMs: performance.now() - startedAt,
                  isPlaying: controller.isPlaying(),
                  error: null,
                });
              } catch (error) {
                reportError("audio-clock animation tick", error);
              }
              return;
            }

            try {
              const startedAt = performance.now();
              const changed = await controller.tickAsync(now, canvas);
              if (changed) {
                const currentFrame = controller.currentFrame();
                preview.update({
                  frame: currentFrame,
                  renderMs: performance.now() - startedAt,
                  isPlaying: controller.isPlaying(),
                  error: null,
                });
              }
            } catch (error) {
              reportError("animation tick", error);
            }
          });
        }
      }

      animationFrameRef.current = requestAnimationFrame(loop);
    };

    animationFrameRef.current = requestAnimationFrame(loop);
  }

  useEffect(() => {
    let cancelled = false;
    const { LumenPreviewController: PreviewController } = createLumenPreviewRuntime(bindings);

    Promise.resolve()
      .then(() => {
        if (cancelled) {
          return;
        }

        const controller = new PreviewController();
        const audioEngine = new LumenAudioEngine();
        compositionLoadedRef.current = false;
        audioEngineRef.current = audioEngine;
        controllerRef.current = controller;
        preview._attach(
          controller,
          (frame) => {
            queueRender(async () => {
              try {
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
        if (mediaSources.length > 0) {
          return controller.syncMediaSources(mediaSources).then(() => {
            if (cancelled) {
              return;
            }
            if (compositionJson) {
              handleLoadComposition(controller, compositionJson, fps);
              queueRender(() => renderOnce(controller));
            }
            startLoop(controller);
          });
        }

        if (compositionJson) {
          handleLoadComposition(controller, compositionJson, fps);
          queueRender(() => renderOnce(controller));
        }
        startLoop(controller);
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          reportError("initialize lumen-preview", error);
        }
      });

    return () => {
      cancelled = true;
      cancelAnimationFrame(animationFrameRef.current);
      animationFrameRef.current = 0;
      audioEngineRef.current?.dispose();
      audioEngineRef.current = null;
      controllerRef.current?.clear();
      controllerRef.current = null;
      compositionLoadedRef.current = false;
      preview._detach();
    };
  }, [preview, bindings, compositionJson, fps, mediaSources]);

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
      compositionLoadedRef.current = false;
      preview.update({ isLoaded: false });
      return;
    }

    let cancelled = false;

    void controller
      .syncMediaSources(mediaSources)
      .then(() => {
        if (cancelled) {
          return;
        }
        handleLoadComposition(controller, compositionJson, fps);
        queueRender(() => renderOnce(controller));
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          reportError("sync media sources", error);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [compositionJson, fps, mediaSources, preview]);

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
