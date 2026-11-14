<script lang="ts">
    import { runtime2d } from "@lumiscia/lumen-bindings/urls-2d";
    import PreviewPlayer from "./PreviewPlayer.svelte";
    import type { PlayerProps } from "./types.js";

    // The smaller 2D-only wasm, which reports 3D nodes as `node.unsupported`.
    let props: PlayerProps = $props();
</script>

<PreviewPlayer {...props} defaultRuntime={runtime2d} />
