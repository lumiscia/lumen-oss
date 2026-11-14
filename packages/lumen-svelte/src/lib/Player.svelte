<script lang="ts">
    import { runtime } from "@lumiscia/lumen-bindings/urls";
    import PreviewPlayer from "./PreviewPlayer.svelte";
    import type { PlayerProps } from "./types.js";

    // The full wasm, which renders 2D and 3D nodes.
    let props: PlayerProps = $props();
</script>

<PreviewPlayer {...props} defaultRuntime={runtime} />
