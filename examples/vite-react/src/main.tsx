import { Composition } from "@lumiscia/lumen-shared";
import { LumenCanvas, createLumenPreview, useLumenPreview } from "@lumiscia/lumen-react";
import * as lumenBindings from "@lumiscia/lumen-bindings/bundler";
import { createRoot } from "react-dom/client";
import "./style.css";

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
  params: {
    width: 1280,
    height: 720,
    color: [25, 28, 36, 255],
  },
});

const title = composition.addNode({
  type: "text",
  params: {
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
  params: {
    opacity: 1,
    blend_mode: "normal",
  },
});

composition.connect(background, merge, { toPort: "base" });
composition.connect(title, merge, { toPort: "overlay" });
const output = composition.addNode({ type: "media_output" });
composition.connect(merge, output, { toPort: "source" });

const preview = createLumenPreview();
const compositionJson = JSON.stringify(composition.toJSON());

function App() {
  const state = useLumenPreview(preview);

  return (
    <main>
      <LumenCanvas preview={preview} bindings={lumenBindings} compositionJson={compositionJson} />
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
