<script lang="ts">
    import { untrack } from "svelte";
    import * as defaults from "@lumiscia/lumen-bindings/urls";
    import { mountPreview, type LumenPreviewSession } from "@lumiscia/lumen-preview";
    import type { PlayerProps } from "./types.js";

    let {
        preview,
        runtimeUrl,
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

    // A URL passed wins, then the file in `runtimeUrl`, then the package's own copy.
    const runtime = $derived(runtimeUrl === undefined ? undefined : String(runtimeUrl));
    const wasm = $derived(
        wasmUrl !== undefined
            ? String(wasmUrl)
            : runtime === undefined
              ? String(defaults.wasmUrl)
              : undefined,
    );
    const worker = $derived(
        workerUrl !== undefined
            ? String(workerUrl)
            : runtime === undefined
              ? String(defaults.workerUrl)
              : undefined,
    );

    let host = $state<HTMLDivElement>();
    let session = $state.raw<LumenPreviewSession>();

    $effect(() => {
        if (!host) {
            return;
        }
        const mounted = mountPreview(host, {
            ...untrack(inputs),
            preview,
            runtimeUrl: runtime,
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
