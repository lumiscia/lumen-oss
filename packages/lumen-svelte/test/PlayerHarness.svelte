<script lang="ts">
    import type { LumenPreviewComposition, LumenWorkerBindings } from "@lumiscia/lumen-preview";

    import Player from "../src/lib/Player.svelte";
    import type { LumenPreviewContext } from "../src/lib/preview.svelte.js";

    type Props = {
        initialPreview: LumenPreviewContext;
        bindings: LumenWorkerBindings;
    };

    let { initialPreview, bindings }: Props = $props();
    const getInitialPreview = () => initialPreview;
    let preview = $state.raw(getInitialPreview());
    let composition = $state.raw<LumenPreviewComposition | null>(null);

    export function setPreview(next: LumenPreviewContext): void {
        preview = next;
    }

    export function setComposition(next: LumenPreviewComposition): void {
        composition = next;
    }
</script>

<Player {preview} {bindings} {composition} class="stage" />
