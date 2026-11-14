import { nodes as n, vec3, type Expr, type NodeValue } from "@lumiscia/lumen-shared";

import { orbit } from "./orbit.js";

/**
 * The glTF fox at `source` walking round the origin, `angle` radians round from +Z and facing the
 * way it walks.
 */
export function fox(source: string, angle: Expr<number>): NodeValue<"scene"> {
  return n.model_3d({
    source,
    animation: "Walk",
    scale: [0.01, 0.01, 0.01],
    position: orbit(angle, 2.9, -0.8),
    rotation: vec3(0, angle.mul(180 / Math.PI).add(90), 0),
  });
}
