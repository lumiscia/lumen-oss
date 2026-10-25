<script lang="ts">
    import { untrack } from "svelte";
    import { LumenPreviewSession } from "@lumiscia/lumen-preview";
    import type {
        AudioSourceRegistration,
        LumenLogLevel,
        LumenPreviewBindingSource,
        LumenPreviewStatsCallback,
        MediaRegistration,
    } from "@lumiscia/lumen-preview";
    import type { LumenPreviewContext } from "./preview.svelte.js";

    type Props = {
        preview: LumenPreviewContext;
        bindings: LumenPreviewBindingSource;
        audioSources?: AudioSourceRegistration[];
        compositionJson?: string | null;
        mediaSources?: MediaRegistration[];
        lookaheadCount?: number;
        logLevel?: LumenLogLevel;
        onStats?: LumenPreviewStatsCallback;
        class?: string;
        style?: string;
    };

    let {
        preview,
        bindings,
        audioSources = [],
        compositionJson = null,
        mediaSources = [],
        lookaheadCount,
        logLevel = "off",
        onStats,
        class: className,
        style: styleName,
    }: Props = $props();

    let canvas = $state<HTMLCanvasElement | null>(null);
    let session: LumenPreviewSession | null = null;

    $effect(() => {
        const activeCanvas = canvas;
        const activePreview = preview;
        const activeBindings = bindings;
        if (!activeCanvas) {
            return;
        }

        const nextSession = untrack(() =>
            new LumenPreviewSession({
                preview: activePreview.core,
                bindings: activeBindings,
                audioSources,
                compositionJson,
                mediaSources,
                ...(lookaheadCount === undefined ? {} : { lookaheadCount }),
                logLevel,
                onStats: onStats ?? null,
            }),
        );
        session = nextSession;
        void nextSession.attach(activeCanvas).catch((error: unknown) => {
            if (session !== nextSession) {
                return;
            }
            activePreview.update({
                error: error instanceof Error ? (error.stack ?? error.message) : String(error),
            });
        });

        return () => {
            nextSession.dispose();
            if (session === nextSession) {
                session = null;
            }
        };
    });

    $effect(() => {
        session?.update({
            audioSources,
            compositionJson,
            mediaSources,
            ...(lookaheadCount === undefined ? {} : { lookaheadCount }),
            logLevel,
            onStats: onStats ?? null,
        });
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
