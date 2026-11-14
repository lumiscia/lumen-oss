import { Player, createLumenPreview, useLumenPreview } from "@lumiscia/lumen-react";
import { useState } from "react";

import { createHello } from "./compositions/hello.js";
import { createScene } from "./compositions/scene.js";
import { fonts, media } from "./media.js";

// Open with `?scene=3d` for the 3D scene.
const composition =
  new URLSearchParams(location.search).get("scene") === "3d" ? createScene() : createHello();
const preview = createLumenPreview();

export function App() {
  const state = useLumenPreview(preview);
  const [loop, setLoop] = useState(false);

  return (
    <main>
      <Player
        preview={preview}
        composition={composition}
        media={media}
        fonts={fonts}
        loop={loop}
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
        <label>
          <input type="checkbox" checked={loop} onChange={(e) => setLoop(e.target.checked)} /> Loop
        </label>
        <span>
          Frame {state.frame} / {state.totalFrames}
        </span>
        {state.error ? <pre>{state.error}</pre> : null}
      </div>
    </main>
  );
}
