<script lang="ts">
    import { Composition } from "@lumiscia/lumen-shared";
    import { Player, createLumenPreview } from "@lumiscia/lumen-svelte";

    import clipUrl from "../../assets/clip.mp4?url";
    import robotoUrl from "../../../crates/text/assets/roboto/Roboto-Regular.ttf?url";

    const composition = new Composition({
        renderSettings: { width: 1280, height: 720 },
        timeline: { fps: 30, durationSeconds: 4 },
    });

    const video = composition.addNode({
        type: "media_in",
        params: { kind: "video", source: "clip" },
    });
    const scaled = composition.addNode({
        type: "resize",
        params: { width: 1280, height: 720, mode: "fill" },
        inputs: { source: video },
    });
    const title = composition.addNode({
        type: "text",
        params: {
            content: "Hello from Lumen",
            font_family: "Roboto",
            font_size: 96,
            color: [255, 255, 255, 255],
            position: [64, 520],
        },
    });
    const merged = composition.addNode({
        type: "merge",
        inputs: { base: scaled, overlay: title },
    });
    composition.addNode({ type: "media_output", inputs: { source: merged } });

    composition
        .addAudioTrack({ id: "clip-audio", name: "Clip audio" })
        .addAudio({ source: "clip", start: 0, duration: 4, trackId: "clip-audio" });

    const preview = createLumenPreview();
    const media = { clip: clipUrl };
    const fonts = { Roboto: robotoUrl };
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
