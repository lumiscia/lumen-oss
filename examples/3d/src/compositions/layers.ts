import { Composition, nodes as n, sin, time, vec3 } from "@lumiscia/lumen-sdk";

import { gradient } from "../components/backdrop.js";
import { hill, layer } from "../components/layers.js";
import { FPS, H, SANS, SECONDS, W, WHITE } from "../theme.js";

/** 2D layers at different depths under a camera that swings and dollies in. */
export function createLayers(): Composition {
  const sky = layer(gradient("#141840", "#f48c60"), 60, [0, 0, -12]);
  const title = layer(
    n.text({
      content: "2.5D",
      font_family: SANS,
      font_weight: 800,
      font_size: 360,
      color: WHITE,
      alignment_horizontal: "center",
      position: [0, 140],
      max_width: W,
    }),
    5,
    [0, 0.2, -2],
  );
  const dot = layer(
    n.shape({
      geometry_kind: "ellipse",
      width: 90,
      height: 90,
      position: [635, 315],
      fill_paint: "#ffffffb4",
    }),
    3,
    [0.3, -0.4, 2.5],
  );
  // The camera swings sideways and dollies in while looking at the title, so nearer layers cross
  // the frame faster than farther ones.
  const camera = n.camera_3d({
    position: vec3(sin(time.mul(1.3)).mul(1.6), 0.4, time.mul(-0.35).add(5)),
    look_at: true,
    target: [0, 0.2, -2],
  });
  return new Composition({
    renderSettings: { width: W, height: H },
    timeline: { fps: FPS, durationSeconds: SECONDS },
  }).addOutput(
    n.render_3d({
      scene: n.merge_3d({
        objects: [
          sky,
          layer(hill("#2e265c", 760), 40, [0, -2.6, -9]),
          layer(hill("#201a46", 800), 30, [0, -2, -6]),
          title,
          layer(hill("#0e0c20", 840), 12, [0, -0.5, -0.5]),
          dot,
          camera,
        ],
      }),
      tone_mapping: "none",
    }),
  );
}
