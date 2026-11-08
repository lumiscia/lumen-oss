<script lang="ts">
    import type { LumenPreviewComposition } from "@lumiscia/lumen-preview";

    import Player from "../src/lib/Player.svelte";
    import type { LumenPreviewContext } from "../src/lib/preview.svelte.js";

    type Props = {
        initialPreview: LumenPreviewContext;
    };

    let { initialPreview }: Props = $props();
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

<Player {preview} {composition} class="stage" />
