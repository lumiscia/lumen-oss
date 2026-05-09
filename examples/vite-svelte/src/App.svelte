<script lang="ts">
    import { Composition } from "@lumiscia/lumen-shared";
    import { LumenCanvas, createLumenPreview } from "@lumiscia/lumen-svelte";
    import * as lumenBindings from "lumen-bindings/bundler";

    const composition = new Composition({
        metadata: {
            name: "Svelte preview example",
        },
        renderSettings: {
            width: 1280,
            height: 720,
            background_color: [18, 20, 26, 255],
        },
        timeline: {
            fps: 30,
            durationSeconds: 4,
        },
    });

    const background = composition.addSolidColor({
        width: 1280,
        height: 720,
        color: [25, 28, 36, 255],
    });

    const title = composition.addText({
        content: "Hello from Lumen Svelte",
        fontFamily: "Inter",
        fontSize: 76,
        fontWeight: 700,
        color: [255, 255, 255, 255],
        maxWidth: 900,
    });

    const merge = composition.addNode({
        type: "merge",
        properties: {
            opacity: 1,
            blend_mode: 0,
        },
    });

    composition.connect(background, merge, { toPort: "base" });
    composition.connect(title, merge, { toPort: "overlay" });
    composition.addOutput(merge);

    const preview = createLumenPreview();
    const compositionJson = JSON.stringify(composition.toJSON());
</script>

<main>
    <LumenCanvas {preview} bindings={lumenBindings} {compositionJson} />
    <div class="controls">
        <button
            type="button"
            disabled={!preview.isLoaded}
            onclick={() => (preview.isPlaying ? preview.pause() : preview.play())}
        >
            {!preview.isLoaded ? "Loading" : preview.isPlaying ? "Pause" : "Play"}
        </button>
        <span>Frame {preview.frame} / {preview.totalFrames}</span>
        {#if preview.error}
            <pre>{preview.error}</pre>
        {/if}
    </div>
</main>
