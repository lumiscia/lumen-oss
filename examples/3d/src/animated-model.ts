import { Composition, cos, nodes as n, sin, time, vec3 } from "@lumiscia/lumen-sdk";

// The glTF sample fox, CC BY 4.0: its credits are in Fox.README.md beside it. Paths are relative
// to the media root, which is the repository root in the commands in the README.
const fox = "crates/engine/tests/fixtures/Fox.glb";

const backdrop = n.background({
  paint: {
    type: "linear",
    start: [0, 0],
    end: [0, 1],
    stops: [
      [0, [32, 44, 64, 255]],
      [1, [92, 112, 128, 255]],
    ],
  },
});

// The model is about 0.95 m tall at this scale and runs in place, so the camera circles it.
const model = n.model_3d({
  source: fox,
  animation: "Run",
  scale: [0.012, 0.012, 0.012],
  rotation: [0, 20, 0],
});

// A floor that shows nothing but the shadows on it, so the fox stands on the backdrop.
const floor = n.shape_3d({
  shape: "plane",
  size: [30, 1, 30],
  shadow_catcher: true,
});

const sun = n.light_3d({
  light: "directional",
  intensity: 3,
  rotation: [-50, -35, 0],
  cast_shadows: true,
  shadow_softness: 0.05,
});
const fill = n.light_3d({ light: "ambient", intensity: 0.5 });

const camera = n.camera_3d({
  position: vec3(sin(time.mul(0.8)).mul(2.6), 0.9, cos(time.mul(0.8)).mul(2.6)),
  look_at: true,
  target: [0, 0.45, 0],
});

export const composition = new Composition({
  renderSettings: { width: 1280, height: 720 },
  timeline: { fps: 30, durationSeconds: 4 },
}).addOutput(
  n.merge({
    base: backdrop,
    overlay: n.render_3d({
      scene: n.merge_3d({ objects: [model, floor, sun, fill, camera] }),
      tone_mapping: "aces",
    }),
  }),
);
