<script lang="ts">
    import { untrack } from "svelte";
    import * as defaults from "@lumiscia/lumen-bindings/urls";
    import { mountPreview, type LumenPreviewSession } from "@lumiscia/lumen-preview";
    import type { PlayerProps } from "./types.js";

    let {
        preview,
        wasmUrl,
        workerUrl,
        composition,
        fonts,
        loop,
        lookaheadCount,
        logLevel,
        media,
        muted,
        onEnded,
        onError,
        onStats,
        onTimeUpdate,
        range,
        rate,
        volume,
        class: className,
        style,
    }: PlayerProps = $props();

    const inputs = () => ({
        composition,
        fonts,
        loop,
        lookaheadCount,
        logLevel,
        media,
        muted,
        onEnded,
        onError,
        onStats,
        onTimeUpdate,
        range,
        rate,
        volume,
    });

    const wasm = $derived(String(wasmUrl ?? defaults.wasmUrl));
    const worker = $derived(String(workerUrl ?? defaults.workerUrl));

    let host = $state<HTMLDivElement>();
    let session = $state.raw<LumenPreviewSession>();

    $effect(() => {
        if (!host) {
            return;
        }
        const mounted = mountPreview(host, {
            ...untrack(inputs),
            preview,
            wasmUrl: wasm,
            workerUrl: worker,
        });
        session = mounted;
        return () => mounted.dispose();
    });

    $effect(() => {
        session?.update(inputs());
    });
</script>

<div bind:this={host} class={className} {style}></div>
