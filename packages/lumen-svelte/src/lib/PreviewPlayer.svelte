<script lang="ts">
    import { untrack } from "svelte";
    import { mountPreview, type LumenPreviewSession, type LumenRuntime } from "@lumiscia/lumen-preview";
    import type { PlayerProps } from "./types.js";

    let {
        defaultRuntime,
        preview,
        runtime,
        runtimeUrl,
        class: className,
        style,
        ...inputs
    }: PlayerProps & { defaultRuntime: LumenRuntime } = $props();

    // By content, so a manifest rebuilt with the same files doesn't restart the preview.
    const runtimeKey = $derived(JSON.stringify(runtime ?? defaultRuntime));
    const directory = $derived(runtimeUrl === undefined ? undefined : String(runtimeUrl));

    let host = $state<HTMLDivElement>();
    let session = $state.raw<LumenPreviewSession>();

    $effect(() => {
        if (!host) {
            return;
        }
        void runtimeKey;
        const mounted = mountPreview(host, {
            ...untrack(() => ({ ...inputs })),
            preview,
            runtime: untrack(() => runtime ?? defaultRuntime),
            runtimeUrl: directory,
        });
        session = mounted;
        return () => mounted.dispose();
    });

    // Runs only when a prop it read changed, and the session reads the whole set again.
    $effect(() => {
        session?.setInputs({ ...inputs });
    });
</script>

<div bind:this={host} class={className} {style}></div>
