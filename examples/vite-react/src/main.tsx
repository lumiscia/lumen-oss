import { Composition } from "@lumiscia/lumen-shared";
import { LumenCanvas, createLumenPreview, useLumenPreview } from "@lumiscia/lumen-react";
import * as lumenBindings from "lumen-bindings/bundler";
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

const background = composition.addSolidColor({
  width: 1280,
  height: 720,
  color: [25, 28, 36, 255],
});

const title = composition.addText({
  content: "Hello from Lumen React",
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

function App() {
  const state = useLumenPreview(preview);

  return (
    <main>
      <LumenCanvas
        preview={preview}
        bindings={lumenBindings}
        compositionJson={compositionJson}
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
