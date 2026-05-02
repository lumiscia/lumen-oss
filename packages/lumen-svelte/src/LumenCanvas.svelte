<script lang="ts">
    import { onMount } from "svelte";
    import type { LumenPreviewController } from "lumen-wasm";
    import type { LumenPreviewContext } from "./preview.svelte.js";

    type Props = {
        preview: LumenPreviewContext;
        compositionJson?: string | null;
        fps?: number;
        class?: string;
        style?: string;
    };

    let {
        preview,
        compositionJson = null,
        fps = 30,
        class: className,
        style: styleName,
    }: Props = $props();

    let canvas = $state<HTMLCanvasElement | null>(null);
    let ctrl = $state<LumenPreviewController | null>(null);
    let rafId = 0;
    let compositionLoaded = false;

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
        if (!compositionLoaded) return;
        try {
            const t0 = performance.now();
            await controller.renderNowAsync(ctx);
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
            compositionLoaded = true;
            preview.totalFrames = controller.durationFrames();
            preview.width = controller.width();
            preview.height = controller.height();
            preview.error = null;
        } catch (e: unknown) {
            compositionLoaded = false;
            reportError("loadComposition", e);
        }
    }

    function startLoop(controller: LumenPreviewController): void {
        cancelAnimationFrame(rafId);
        rafId = 0;

        function loop(now: number): void {
            rafId = 0;
            const ctx = getCtx2d();

            if (ctx && compositionLoaded && controller.isPlaying()) {
                queueRender(async () => {
                    if (!compositionLoaded) {
                        return;
                    }

                    try {
                        const t0 = performance.now();
                        const changed = await controller.tickAsync(now, ctx);
                        if (changed) {
                            preview.renderMs = performance.now() - t0;
                            preview.frame = controller.currentFrame();
                            preview.isPlaying = controller.isPlaying();
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
                compositionLoaded = false;
                preview._attach(controller, (frame) => {
                    queueRender(async () => {
                        try {
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
            compositionLoaded = false;
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
        if (!controller || !json) {
            compositionLoaded = false;
            return;
        }
        handleLoadComposition(controller, json, f);
        queueRender(() => renderOnce(controller));
    });
</script>

<canvas
    bind:this={canvas}
    width={preview.width || 1}
    height={preview.height || 1}
    style={styleName}
    style:visibility={preview.width > 0 ? null : "hidden"}
    class={className}
></canvas>
