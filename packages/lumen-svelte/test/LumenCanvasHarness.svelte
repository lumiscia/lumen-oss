<script lang="ts">
    import type { LumenPreviewBindingSource } from "@lumiscia/lumen-preview";

    import LumenCanvas from "../src/lib/LumenCanvas.svelte";
    import type { LumenPreviewContext } from "../src/lib/preview.svelte.js";

    type Props = {
        initialPreview: LumenPreviewContext;
        initialBindings: LumenPreviewBindingSource;
    };

    let { initialPreview, initialBindings }: Props = $props();
    const getInitialPreview = () => initialPreview;
    const getInitialBindings = () => initialBindings;
    let preview = $state.raw(getInitialPreview());
    let bindings = $state.raw(getInitialBindings());
    let compositionJson = $state<string | null>(null);

    export function replace(
        nextPreview: LumenPreviewContext,
        nextBindings: LumenPreviewBindingSource,
        nextCompositionJson: string,
    ): void {
        preview = nextPreview;
        bindings = nextBindings;
        compositionJson = nextCompositionJson;
    }
</script>

<LumenCanvas {preview} {bindings} {compositionJson} />
