import { Composition } from "@lumiscia/lumen-shared";
import { Player, createLumenPreview, useLumenPreview } from "@lumiscia/lumen-react";
import { createLumenBindings } from "@lumiscia/lumen-bindings/bundler";
import { createRoot } from "react-dom/client";

import clipUrl from "../../assets/clip.mp4?url";
import robotoUrl from "../../../crates/text/assets/roboto/Roboto-Regular.ttf?url";
import "./style.css";

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
const bindings = createLumenBindings();
const media = { clip: clipUrl };
const fonts = { Roboto: robotoUrl };

function App() {
  const state = useLumenPreview(preview);

  return (
    <main>
      <Player
        preview={preview}
        bindings={bindings}
        composition={composition}
        media={media}
        fonts={fonts}
        className="stage"
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
