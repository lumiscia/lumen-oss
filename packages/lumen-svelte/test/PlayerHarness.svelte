<script lang="ts">
    import type { LumenPreviewComposition } from "@lumiscia/lumen-preview";

    import Player from "../src/lib/Player.svelte";
    import type { LumenPreviewContext } from "../src/lib/preview.svelte.js";
    import type { PlayerProps } from "../src/lib/types.js";

    type Props = {
        initialOptions?: Partial<PlayerProps>;
        initialPreview: LumenPreviewContext;
    };

    let { initialOptions = {}, initialPreview }: Props = $props();
    const getInitialOptions = () => initialOptions;
    const getInitialPreview = () => initialPreview;
    let preview = $state.raw(getInitialPreview());
    let composition = $state.raw<LumenPreviewComposition | null>(null);
    let options = $state.raw<Partial<PlayerProps>>(getInitialOptions());

    export function setPreview(next: LumenPreviewContext): void {
        preview = next;
    }

    export function setComposition(next: LumenPreviewComposition): void {
        composition = next;
    }

    export function setOptions(next: Partial<PlayerProps>): void {
        options = next;
    }
</script>

<Player
    {preview}
    {composition}
    class="stage"
    loop={options.loop}
    muted={options.muted}
    onEnded={options.onEnded}
    onTimeUpdate={options.onTimeUpdate}
    range={options.range}
    rate={options.rate}
    volume={options.volume}
/>
