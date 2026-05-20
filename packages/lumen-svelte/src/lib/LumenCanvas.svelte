<script lang="ts">
    import { onMount } from "svelte";
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
        logLevel = "off",
        onStats,
        class: className,
        style: styleName,
    }: Props = $props();

    let canvas = $state<HTMLCanvasElement | null>(null);
    let session: LumenPreviewSession | null = null;

    onMount(() => {
        const nextSession = new LumenPreviewSession({
            preview: preview.core,
            bindings,
            audioSources,
            compositionJson,
            mediaSources,
            logLevel,
            onStats: onStats ?? null,
        });
        session = nextSession;
        void nextSession.attach(canvas).catch((error: unknown) => {
            preview.update({
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
