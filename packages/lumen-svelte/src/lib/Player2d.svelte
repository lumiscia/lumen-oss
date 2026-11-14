<script lang="ts">
    import * as urls from "@lumiscia/lumen-bindings/urls-2d";
    import PreviewPlayer from "./PreviewPlayer.svelte";
    import type { PlayerProps } from "./types.js";

    // The smaller 2D-only wasm, which reports 3D nodes as `node.unsupported`.
    const defaults = { ...urls, wasmFile: "lumen_wasm_2d_bg.wasm", workerFile: "worker-2d.js" };
    let props: PlayerProps = $props();
</script>

<PreviewPlayer {...props} {defaults} />
