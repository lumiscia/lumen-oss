<script lang="ts">
    import { untrack } from "svelte";
    import { mountPreview, type LumenPreviewSession } from "@lumiscia/lumen-preview";
    import type { LumenCanvasProps } from "./types.js";

    let {
        preview,
        bindings,
        composition,
        fonts,
        lookaheadCount,
        logLevel,
        media,
        onStats,
        class: className,
        style,
    }: LumenCanvasProps = $props();

    const inputs = () => ({ composition, fonts, lookaheadCount, logLevel, media, onStats });

    let host = $state<HTMLDivElement>();
    let session = $state.raw<LumenPreviewSession>();

    $effect(() => {
        if (!host) {
            return;
        }
        const mounted = mountPreview(host, { ...untrack(inputs), bindings, preview });
        session = mounted;
        return () => mounted.dispose();
    });

    $effect(() => {
        session?.update(inputs());
    });
</script>

<div bind:this={host} class={className} {style}></div>
