import { Composition, cos, nodes as n, sin, time, vec3 } from "@lumiscia/lumen-sdk";

import { gradient } from "../components/backdrop.js";
import { runningFox } from "../components/fox.js";
import { FPS, H, SECONDS, W } from "../theme.js";

/** The glTF fox running over a shadow catcher, with an orbiting camera. */
export function createAnimatedModel(): Composition {
  // A floor that shows nothing but the shadows on it, so the fox stands on the backdrop.
  const floor = n.shape_3d({ shape: "plane", size: [30, 1, 30], shadow_catcher: true });
  const sun = n.light_3d({
    light: "directional",
    intensity: 3,
    rotation: [-50, -35, 0],
    cast_shadows: true,
    shadow_softness: 0.05,
  });
  const fill = n.light_3d({ light: "ambient", intensity: 0.5 });
  // The fox runs in place, so the camera circles it.
  const camera = n.camera_3d({
    position: vec3(sin(time.mul(0.8)).mul(2.6), 0.9, cos(time.mul(0.8)).mul(2.6)),
    look_at: true,
    target: [0, 0.45, 0],
  });
  return new Composition({
    renderSettings: { width: W, height: H },
    timeline: { fps: FPS, durationSeconds: SECONDS },
  }).addOutput(
    n.merge({
      base: gradient("#202c40", "#5c7080"),
      overlay: n.render_3d({
        scene: n.merge_3d({ objects: [runningFox(), floor, sun, fill, camera] }),
        tone_mapping: "aces",
      }),
    }),
  );
}
