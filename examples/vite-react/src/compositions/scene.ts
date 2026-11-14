import { Composition, nodes as n, time } from "@lumiscia/lumen-shared";

import { fox } from "../components/fox.js";
import { orbit } from "../components/orbit.js";
import { screen } from "../components/screen.js";
import { cube, floor, glass, ring } from "../components/shapes.js";
import { CLIP, FOX, FPS, H, SECONDS, SKY, W } from "../theme.js";

/** Shapes, the clip on a plane and an animated glTF model, lit and drawn through a camera. */
export function createScene(): Composition {
  // The camera circles the shapes, and the fox walks round them on the far side from it.
  const turn = time.mul(0.4);
  const camera = n.camera_3d({ look_at: true, position: orbit(turn, 5, 1.4) });
  const sun = n.light_3d({ light: "directional", intensity: 3, rotation: [-35, 40, 0] });
  const fill = n.light_3d({ light: "ambient", intensity: 0.4 });
  return new Composition({
    renderSettings: { width: W, height: H },
    timeline: { fps: FPS, durationSeconds: SECONDS },
  }).addOutput(
    n.merge({
      base: n.background({ paint: SKY }),
      overlay: n.render_3d({
        tone_mapping: "agx",
        scene: n.merge_3d({
          objects: [
            floor(),
            ring(),
            cube(),
            glass(),
            screen(CLIP),
            fox(FOX, turn.add(2.2)),
            camera,
            sun,
            fill,
          ],
        }),
      }),
    }),
  );
}
