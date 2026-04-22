<script lang="ts">
    import { onMount } from "svelte";
    import type { LumenPreviewController } from "lumen-wasm";
    import type { LumenPreviewContext } from "./preview.svelte.js";

    type Props = {
        preview: LumenPreviewContext;
        compositionJson?: string | null;
        fps?: number;
        class?: string;
    };

    let {
        preview,
        compositionJson = null,
        fps = 30,
        class: className,
    }: Props = $props();

    let canvas = $state<HTMLCanvasElement | null>(null);
    let ctrl = $state<LumenPreviewController | null>(null);
    let rafId = 0;
    let prefetchGeneration = 0;
    const prefetches = new Map<number, Promise<void>>();
    const PREFETCH_AHEAD_FRAMES = 6;

    function getCtx2d(): CanvasRenderingContext2D | null {
        return canvas?.getContext("2d") ?? null;
    }

    let renderInFlight = false;
    let queuedRender: (() => Promise<void>) | null = null;

    function describeError(error: unknown): string {
        if (error instanceof Error) {
            return error.stack ?? `${error.name}: ${error.message}`;
        }
        return String(error);
    }

    function reportError(scope: string, error: unknown): void {
        console.error(`[LumenCanvas] ${scope}`, error);
        preview.error = describeError(error);
    }

    function normalizeFrame(frame: number): number {
        const totalFrames = preview.totalFrames;
        if (totalFrames <= 0) {
            return frame;
        }

        return ((frame % totalFrames) + totalFrames) % totalFrames;
    }

    async function preloadFrame(controller: LumenPreviewController, frame: number): Promise<void> {
        const totalFrames = preview.totalFrames;
        const normalizedFrame = normalizeFrame(frame);
        if (normalizedFrame < 0 || (totalFrames > 0 && normalizedFrame >= totalFrames)) {
            return;
        }

        const existing = prefetches.get(normalizedFrame);
        if (existing) {
            await existing;
            return;
        }

        const generation = prefetchGeneration;
        const pending = controller.preloadFrame(normalizedFrame).finally(() => {
            if (prefetches.get(normalizedFrame) === pending) {
                prefetches.delete(normalizedFrame);
            }
        });
        prefetches.set(normalizedFrame, pending);

        try {
            await pending;
        } catch (error) {
            if (generation !== prefetchGeneration) {
                return;
            }
            throw error;
        }
    }

    function resetPrefetch(): void {
        prefetchGeneration += 1;
        prefetches.clear();
    }

    function prefetchAhead(controller: LumenPreviewController, frame: number): void {
        for (let offset = 1; offset <= PREFETCH_AHEAD_FRAMES; offset += 1) {
            void preloadFrame(controller, frame + offset).catch(() => {
                // The render path will surface any persistent media errors.
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
        queuedRender = operation;
        if (renderInFlight) {
            return;
        }
        void drainRenderQueue();
    }

    async function drainRenderQueue(): Promise<void> {
        renderInFlight = true;
        try {
            while (queuedRender) {
                const next = queuedRender;
                queuedRender = null;
                await next();
            }
        } finally {
            renderInFlight = false;
            if (queuedRender) {
                void drainRenderQueue();
            }
        }
    }

    async function renderOnce(controller: LumenPreviewController): Promise<void> {
        const ctx = getCtx2d();
        if (!ctx) return;
        try {
            await preloadPlaybackWindow(controller, controller.currentFrame());
            const t0 = performance.now();
            controller.renderNow(ctx);
            preview.renderMs = performance.now() - t0;
            preview.frame = controller.currentFrame();
            preview.isPlaying = controller.isPlaying();
            preview.error = null;
        } catch (e: unknown) {
            reportError("renderOnce", e);
        }
    }

    function handleLoadComposition(
        controller: LumenPreviewController,
        json: string,
        f: number,
    ): void {
        try {
            controller.loadComposition(json, f);
            preview.totalFrames = controller.durationFrames();
            preview.width = controller.width();
            preview.height = controller.height();
            preview.error = null;
        } catch (e: unknown) {
            reportError("loadComposition", e);
        }
    }

    function startLoop(controller: LumenPreviewController): void {
        cancelAnimationFrame(rafId);
        rafId = 0;

        function loop(now: number): void {
            rafId = 0;
            const ctx = getCtx2d();

            if (ctx) {
                queueRender(async () => {
                    try {
                        const currentFrame = controller.currentFrame();
                        await preloadPlaybackWindow(controller, currentFrame);
                    } catch {
                        // Keep the animation loop alive while media settles.
                    }

                    try {
                        const t0 = performance.now();
                        const changed = controller.tick(now, ctx);
                        if (changed) {
                            preview.renderMs = performance.now() - t0;
                            preview.frame = controller.currentFrame();
                            preview.isPlaying = controller.isPlaying();
                            prefetchAhead(controller, preview.frame);
                        }
                        preview.error = null;
                    } catch (e: unknown) {
                        reportError("animation tick", e);
                        // Don't return — keep the loop alive so transient errors
                        // (e.g. media not yet loaded) recover automatically.
                    }
                });
            }

            rafId = requestAnimationFrame(loop);
        }

        rafId = requestAnimationFrame(loop);
    }

    onMount(() => {
        let cancelled = false;

        import("lumen-wasm")
            .then(({ LumenPreviewController }) => {
                if (cancelled) return;
                const controller = new LumenPreviewController();
                preview._attach(controller, (frame) => {
                    queueRender(async () => {
                        try {
                            resetPrefetch();
                            controller.setFrame(frame);
                            await renderOnce(controller);
                        } catch (e: unknown) {
                            reportError("seek render", e);
                        }
                    });
                });
                ctrl = controller;
                startLoop(controller);
            })
            .catch((e: unknown) => {
                if (!cancelled) {
                    reportError("import lumen-wasm", e);
                }
            });

        return () => {
            cancelled = true;
            cancelAnimationFrame(rafId);
            rafId = 0;
            resetPrefetch();
            ctrl?.clear();
            preview._detach();
            ctrl = null;
        };
    });

    // Load/reload composition when compositionJson, fps, or ctrl changes
    $effect(() => {
        const json = compositionJson;
        const f = fps;
        const controller = ctrl;
        if (!controller || !json) return;
        resetPrefetch();
        handleLoadComposition(controller, json, f);
        queueRender(() => renderOnce(controller));
    });
</script>

<canvas
    bind:this={canvas}
    width={preview.width || 1}
    height={preview.height || 1}
    style:visibility={preview.width > 0 ? null : "hidden"}
    class={className}
></canvas>
