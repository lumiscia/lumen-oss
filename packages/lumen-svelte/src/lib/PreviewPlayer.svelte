<script lang="ts">
    import { untrack } from "svelte";
    import { mountPreview, runtimeFileUrl, type LumenPreviewSession } from "@lumiscia/lumen-preview";
    import type { PlayerDefaults, PlayerProps } from "./types.js";

    let {
        defaults,
        preview,
        runtimeUrl,
        wasmUrl,
        workerUrl,
        class: className,
        style,
        ...inputs
    }: PlayerProps & { defaults: PlayerDefaults } = $props();

    // A URL passed wins, then the file in `runtimeUrl`, then the package's own copy.
    const runtime = $derived(runtimeUrl === undefined ? undefined : String(runtimeUrl));
    const wasm = $derived(
        wasmUrl !== undefined
            ? String(wasmUrl)
            : runtime === undefined
              ? String(defaults.wasmUrl)
              : runtimeFileUrl(runtime, defaults.wasmFile),
    );
    const worker = $derived(
        workerUrl !== undefined
            ? String(workerUrl)
            : runtime === undefined
              ? String(defaults.workerUrl)
              : runtimeFileUrl(runtime, defaults.workerFile),
    );

    let host = $state<HTMLDivElement>();
    let session = $state.raw<LumenPreviewSession>();

    $effect(() => {
        if (!host) {
            return;
        }
        const mounted = mountPreview(host, {
            ...untrack(() => ({ ...inputs })),
            preview,
            runtimeUrl: runtime,
            wasmUrl: wasm,
            workerUrl: worker,
        });
        session = mounted;
        return () => mounted.dispose();
    });

    // The session ignores inputs that haven't changed, so every change can pass them all on.
    $effect(() => {
        session?.setInputs({ ...inputs });
    });
</script>

<div bind:this={host} class={className} {style}></div>
