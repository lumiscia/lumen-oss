import { frame, nodes as n, vec3, type NodeValue } from "@lumiscia/lumen-shared";

import { BLUE, GOLD, RED } from "../theme.js";

type Scene = NodeValue<"scene">;

export function floor(): Scene {
  return n.shape_3d({ shape: "plane", size: [8, 1, 8], position: [0, -0.8, 0], roughness: 0.9 });
}

/** A gold torus, tipped back and turning. */
export function ring(): Scene {
  return n.shape_3d({
    shape: "torus",
    size: [1.6, 1.6, 1.6],
    rotation: vec3(70, frame.mul(3), 0),
    color: GOLD,
    metallic: 1,
    roughness: 0.25,
  });
}

/** A red cube on the left, turning the other way. */
export function cube(): Scene {
  return n.shape_3d({
    position: [-1.9, -0.4, 0.3],
    size: [0.8, 0.8, 0.8],
    rotation: vec3(0, frame.mul(-2), 0),
    color: RED,
    roughness: 0.4,
  });
}

/** A see-through blue sphere on the right. */
export function glass(): Scene {
  return n.shape_3d({
    shape: "sphere",
    position: [1.9, -0.3, 0.3],
    color: BLUE,
    opacity: 0.6,
    roughness: 0.1,
  });
}
