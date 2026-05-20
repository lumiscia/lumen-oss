import { Composition } from "@lumiscia/lumen-shared";
import { LumenCanvas, createLumenPreview, useLumenPreview } from "@lumiscia/lumen-react";
import { createLumenBindings } from "@lumiscia/lumen-bindings/bundler";
import type { AudioSourceRegistration, LumenPreviewStats } from "@lumiscia/lumen-react";
import { useCallback, useState } from "react";
import { createRoot } from "react-dom/client";
import "./style.css";

const AUDIO_SOURCE_ID = "demo-tone";
const AUDIO_TRACK_ID = "music";
const SAMPLE_RATE = 48_000;
const CHANNEL_COUNT = 2;
const FPS_SMOOTHING = 0.18;
const EMPTY_STATS: LumenPreviewStats = {
  frame: 0,
  timelineFps: 0,
  targetFrameDurationMs: 0,
  renderMs: 0,
  actualFps: 0,
};

const composition = new Composition({
  metadata: {
    name: "React preview example",
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
    content: "Hello from Lumen React",
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
const audioSources: AudioSourceRegistration[] = [
  {
    id: AUDIO_SOURCE_ID,
    kind: "audio",
    source: createDemoWavBlob(4),
  },
];
const compositionJson = JSON.stringify({
  ...composition.toJSON(),
  audio: {
    tracks: [
      {
        id: AUDIO_TRACK_ID,
        name: "Generated tone",
        muted: false,
        solo: false,
        volume: 0.42,
      },
    ],
    clips: [
      {
        id: "intro-chord",
        source_id: AUDIO_SOURCE_ID,
        track_id: AUDIO_TRACK_ID,
        name: "Generated tone",
        start_ms: 0,
        duration_ms: 4_000,
        source_start_ms: 0,
        volume: 1,
      },
    ],
  },
});

function App() {
  const state = useLumenPreview(preview);
  const [stats, setStats] = useState(EMPTY_STATS);
  const updateStats = useCallback((nextStats: LumenPreviewStats) => {
    setStats((previousStats) => stabilizePreviewStats(previousStats, nextStats));
  }, []);

  return (
    <main>
      <LumenCanvas
        preview={preview}
        bindings={lumenBindings}
        compositionJson={compositionJson}
        audioSources={audioSources}
        onStats={updateStats}
      />
      <div className="controls">
        <button
          type="button"
          disabled={!state.isLoaded}
          onClick={() => (state.isPlaying ? preview.pause() : preview.play())}
        >
          {!state.isLoaded ? "Loading" : state.isPlaying ? "Pause" : "Play"}
        </button>
        <span>
          Frame {state.frame} / {state.totalFrames}
        </span>
        <span>
          Timeline {stats.timelineFps} fps · target {stats.targetFrameDurationMs.toFixed(2)} ms
        </span>
        <span>
          Render {stats.renderMs.toFixed(2)} ms · actual {stats.actualFps.toFixed(1)} fps
        </span>
        <span className="audio-pill">AudioWorklet + WASM tone</span>
        {state.error ? <pre>{state.error}</pre> : null}
      </div>
    </main>
  );
}

const root = document.getElementById("root");
if (!root) {
  throw new Error("Missing #root element");
}

createRoot(root).render(<App />);

function stabilizePreviewStats(
  previousStats: LumenPreviewStats,
  nextStats: LumenPreviewStats,
): LumenPreviewStats {
  if (nextStats.timelineFps <= 0) {
    return EMPTY_STATS;
  }

  if (nextStats.actualFps <= 0) {
    return {
      ...nextStats,
      actualFps: previousStats.actualFps,
    };
  }

  if (previousStats.actualFps <= 0) {
    return nextStats;
  }

  return {
    ...nextStats,
    actualFps:
      previousStats.actualFps + (nextStats.actualFps - previousStats.actualFps) * FPS_SMOOTHING,
  };
}

function createDemoWavBlob(durationSeconds: number): Blob {
  const frameCount = Math.floor(durationSeconds * SAMPLE_RATE);
  const bytesPerSample = 2;
  const dataBytes = frameCount * CHANNEL_COUNT * bytesPerSample;
  const bytes = new ArrayBuffer(44 + dataBytes);
  const view = new DataView(bytes);
  let offset = 0;

  offset = writeAscii(view, offset, "RIFF");
  view.setUint32(offset, 36 + dataBytes, true);
  offset += 4;
  offset = writeAscii(view, offset, "WAVE");
  offset = writeAscii(view, offset, "fmt ");
  view.setUint32(offset, 16, true);
  offset += 4;
  view.setUint16(offset, 1, true);
  offset += 2;
  view.setUint16(offset, CHANNEL_COUNT, true);
  offset += 2;
  view.setUint32(offset, SAMPLE_RATE, true);
  offset += 4;
  view.setUint32(offset, SAMPLE_RATE * CHANNEL_COUNT * bytesPerSample, true);
  offset += 4;
  view.setUint16(offset, CHANNEL_COUNT * bytesPerSample, true);
  offset += 2;
  view.setUint16(offset, bytesPerSample * 8, true);
  offset += 2;
  offset = writeAscii(view, offset, "data");
  view.setUint32(offset, dataBytes, true);
  offset += 4;

  for (let frame = 0; frame < frameCount; frame += 1) {
    const time = frame / SAMPLE_RATE;
    const envelope = Math.min(1, time * 8, (durationSeconds - time) * 6);
    const phrase = Math.floor(time * 2) % 4;
    const root = [220, 261.63, 293.66, 329.63][phrase] ?? 220;
    const left = tone(time, root) * envelope;
    const right = tone(time, root * 1.5) * envelope;
    view.setInt16(offset, floatToPcm16(left), true);
    offset += 2;
    view.setInt16(offset, floatToPcm16(right), true);
    offset += 2;
  }

  return new Blob([bytes], { type: "audio/wav" });
}

function tone(time: number, frequency: number): number {
  const fundamental = Math.sin(time * frequency * Math.PI * 2);
  const shimmer = Math.sin(time * frequency * 2.01 * Math.PI * 2) * 0.35;
  return (fundamental + shimmer) * 0.38;
}

function floatToPcm16(sample: number): number {
  const clamped = Math.max(-1, Math.min(1, sample));
  return clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff;
}

function writeAscii(view: DataView, offset: number, value: string): number {
  for (let index = 0; index < value.length; index += 1) {
    view.setUint8(offset + index, value.charCodeAt(index));
  }
  return offset + value.length;
}
