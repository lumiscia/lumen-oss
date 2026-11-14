import { Composition, nodes as n } from "@lumiscia/lumen-sdk";

import { screen } from "../components/screen.js";
import { CLIP } from "../media.js";
import { FPS, H, SECONDS, W } from "../theme.js";

/** The clip on the material of a model, a sheet that swings about its vertical axis. */
export function createVideoOnModel(): Composition {
  return new Composition({
    renderSettings: { width: W, height: H },
    timeline: { fps: FPS, durationSeconds: SECONDS },
  }).addOutput(
    n.merge({
      base: n.background({ paint: "#121620" }),
      overlay: n.render_3d({
        scene: n.merge_3d({
          objects: [screen(CLIP), n.camera_3d({ position: [0, 0, 5], look_at: true })],
        }),
      }),
    }),
  );
}
