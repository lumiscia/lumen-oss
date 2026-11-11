import { Composition } from "@lumiscia/lumen-shared";
import { Player, createLumenPreview, useLumenPreview } from "@lumiscia/lumen-react";
import { createRoot } from "react-dom/client";

import clipUrl from "../../assets/clip.mp4?url";
import robotoUrl from "../../../crates/text/assets/roboto/Roboto-Regular.ttf?url";
// The glTF sample fox, CC BY 4.0: its credits are in Fox.README.md beside it.
import foxUrl from "../../../crates/engine/tests/fixtures/Fox.glb?url";
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

// Open with `?scene=3d` for a 3D scene: shapes, a video on a plane, an animated glTF model, a light
// and a camera drawn by render_3d.
const scene3d = new Composition({
  renderSettings: { width: 1280, height: 720 },
  timeline: { fps: 30, durationSeconds: 4 },
});
const sky = scene3d.addNode({ type: "background", params: { paint: [24, 26, 34, 255] } });
const floor = scene3d.addNode({
  type: "shape_3d",
  params: { shape: "plane", size: [8, 1, 8], position: [0, -0.8, 0], roughness: 0.9 },
});
const ring = scene3d.addNode({
  type: "shape_3d",
  params: {
    shape: "torus",
    size: [1.6, 1.6, 1.6],
    rotation: "=vec3(70, frame * 3, 0)",
    color: [230, 180, 90, 255],
    metallic: 1,
    roughness: 0.25,
  },
});
const cube = scene3d.addNode({
  type: "shape_3d",
  params: {
    position: [-1.9, -0.4, 0.3],
    size: [0.8, 0.8, 0.8],
    rotation: "=vec3(0, frame * -2, 0)",
    color: [230, 80, 60, 255],
    roughness: 0.4,
  },
});
const glass = scene3d.addNode({
  type: "shape_3d",
  params: {
    shape: "sphere",
    position: [1.9, -0.3, 0.3],
    color: [90, 160, 255, 255],
    opacity: 0.6,
    roughness: 0.1,
  },
});
const screenVideo = scene3d.addNode({
  type: "media_in",
  params: { kind: "video", source: "clip" },
});
const screenSource = scene3d.addNode({
  type: "resize",
  params: { width: 1280, height: 720, mode: "fill" },
  inputs: { source: screenVideo },
});
const screen = scene3d.addNode({
  type: "image_plane_3d",
  params: { width: 4, position: [0, 1.3, -2.5] },
  inputs: { source: screenSource },
});
const fox = scene3d.addNode({
  type: "model_3d",
  params: {
    source: "fox",
    animation: "Walk",
    scale: [0.01, 0.01, 0.01],
    // Round the shapes on the far side from the camera, facing the way it walks.
    position: "=vec3(2.9 * sin(time * 0.4 + 2.2), -0.8, 2.9 * cos(time * 0.4 + 2.2))",
    rotation: "=vec3(0, (time * 0.4 + 2.2) * 57.3 + 90, 0)",
  },
});
const camera = scene3d.addNode({
  type: "camera_3d",
  params: { look_at: true, position: "=vec3(5 * sin(time * 0.4), 1.4, 5 * cos(time * 0.4))" },
});
const sun = scene3d.addNode({
  type: "light_3d",
  params: { light: "directional", intensity: 3, rotation: [-35, 40, 0] },
});
const fill = scene3d.addNode({ type: "light_3d", params: { light: "ambient", intensity: 0.4 } });
const objects = scene3d.addNode({
  type: "merge_3d",
  inputs: { objects: [floor, ring, cube, glass, screen, fox, camera, sun, fill] },
});
const rendered = scene3d.addNode({
  type: "render_3d",
  params: { tone_mapping: "agx" },
  inputs: { scene: objects },
});
const framed = scene3d.addNode({ type: "merge", inputs: { base: sky, overlay: rendered } });
scene3d.addNode({ type: "media_output", inputs: { source: framed } });

const is3d = new URLSearchParams(location.search).get("scene") === "3d";
const preview = createLumenPreview();
const media = is3d ? { clip: clipUrl, fox: foxUrl } : { clip: clipUrl };
const fonts = { Roboto: robotoUrl };

function App() {
  const state = useLumenPreview(preview);

  return (
    <main>
      <Player
        preview={preview}
        composition={is3d ? scene3d : composition}
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
