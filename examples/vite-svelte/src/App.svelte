<script lang="ts">
    import { Composition } from "@lumiscia/lumen-shared";
    import { LumenCanvas, createLumenPreview } from "@lumiscia/lumen-svelte";
    import { createLumenBindings } from "@lumiscia/lumen-bindings/bundler";
    import type { LumenPreviewStats } from "@lumiscia/lumen-svelte";

    const EMPTY_STATS: LumenPreviewStats = {
        frame: 0,
        timelineFps: 0,
        targetFrameDurationMs: 0,
        renderMs: 0,
        actualFps: 0,
    };

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

    const background = composition.addNode({
        type: "solid_color",
        properties: {
            width: 1280,
            height: 720,
            color: [25, 28, 36, 255],
        },
    });

    const title = composition.addNode({
        type: "text",
        properties: {
            content: "Hello from Lumen Svelte",
            font_family: "Inter",
            font_size: 76,
            font_weight: 700,
            color: [255, 255, 255, 255],
            max_width: 900,
        },
    });

    const merge = composition.addNode({
        type: "merge",
        properties: {
            opacity: 1,
            blend_mode: "normal",
        },
    });

    composition.connect(background, merge, { toPort: "base" });
    composition.connect(title, merge, { toPort: "overlay" });
    const output = composition.addNode({ type: "media_output" });
    composition.connect(merge, output, { toPort: "source" });

    const preview = createLumenPreview();
    const lumenBindings = createLumenBindings();
    const compositionJson = JSON.stringify(composition.toJSON());
    let stats = $state(EMPTY_STATS);
</script>

<main>
    <LumenCanvas {preview} bindings={lumenBindings} {compositionJson} onStats={(next) => (stats = next)} />
    <div class="controls">
        <button
            type="button"
            disabled={!preview.isLoaded}
            onclick={() => (preview.isPlaying ? preview.pause() : preview.play())}
        >
            {!preview.isLoaded ? "Loading" : preview.isPlaying ? "Pause" : "Play"}
        </button>
        <span>Frame {preview.frame} / {preview.totalFrames}</span>
        <span>Timeline {stats.timelineFps} fps · target {stats.targetFrameDurationMs.toFixed(2)} ms</span>
        <span>Render {stats.renderMs.toFixed(2)} ms · actual {stats.actualFps.toFixed(1)} fps</span>
        {#if preview.error}
            <pre>{preview.error}</pre>
        {/if}
    </div>
</main>
