<script lang="ts">
    import { Player, createLumenPreview } from "@lumiscia/lumen-svelte";

    import { createHello } from "./compositions/hello.js";
    import { fonts, media } from "./media.js";

    const composition = createHello();
    const preview = createLumenPreview();
    let loop = $state(false);
</script>

<main>
    <Player {preview} {composition} {media} {fonts} {loop} class="stage" />
    <div class="controls">
        <button
            type="button"
            disabled={!preview.state.isLoaded}
            onclick={() => (preview.state.isPlaying ? preview.pause() : preview.play())}
        >
            {!preview.state.isLoaded ? "Loading" : preview.state.isPlaying ? "Pause" : "Play"}
        </button>
        <label><input type="checkbox" bind:checked={loop} /> Loop</label>
        <span>Frame {preview.state.frame} / {preview.state.totalFrames}</span>
        {#if preview.state.error}
            <pre>{preview.state.error}</pre>
        {/if}
    </div>
</main>
