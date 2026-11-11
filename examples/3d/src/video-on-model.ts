import { Composition, nodes as n, sin, time, vec3 } from "@lumiscia/lumen-sdk";

// A 9 × 9 sheet written for the model reader's tests and licensed like this repository
// (Apache-2.0). It is 2 units wide and 2 tall, with one material named `grid`.
const sheet = "crates/3d/tests/fixtures/grid.glb";
const clip = "examples/assets/clip.mp4";

// The video replaces the sheet's `grid` material, unlit, as an image plane shows a raster. The
// sheet is stretched to the clip's 16:9 and swings about its vertical axis.
const screen = n.model_3d({
  source: sheet,
  texture_material: "grid",
  position: [0, -1, 0],
  rotation: vec3(0, sin(time.mul(1.2)).mul(28), 0),
  scale: [1.78, 1, 1],
  texture: n.resize({
    width: 1280,
    height: 720,
    mode: "fill",
    source: n.media_in({ kind: "video", source: clip }),
  }),
});

export const composition = new Composition({
  renderSettings: { width: 1280, height: 720 },
  timeline: { fps: 30, durationSeconds: 4 },
}).addOutput(
  n.merge({
    base: n.background({ paint: [18, 22, 32, 255] }),
    overlay: n.render_3d({
      scene: n.merge_3d({
        objects: [screen, n.camera_3d({ position: [0, 0, 5], look_at: true })],
      }),
    }),
  }),
);
