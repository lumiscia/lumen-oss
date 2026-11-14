<script lang="ts">
    import * as urls from "@lumiscia/lumen-bindings/urls";
    import PreviewPlayer from "./PreviewPlayer.svelte";
    import type { PlayerProps } from "./types.js";

    // The full wasm, which renders 2D and 3D nodes.
    const defaults = { ...urls, wasmFile: "lumen_wasm_bg.wasm", workerFile: "worker.js" };
    let props: PlayerProps = $props();
</script>

<PreviewPlayer {...props} {defaults} />
