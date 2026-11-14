import { nodes as n, type NodeValue } from "@lumiscia/lumen-sdk";

import { FOX } from "../media.js";

/** The glTF fox running its `Run` clip in place, about 0.95 m tall at this scale. */
export function runningFox(): NodeValue<"scene"> {
  return n.model_3d({
    source: FOX,
    animation: "Run",
    scale: [0.012, 0.012, 0.012],
    rotation: [0, 20, 0],
  });
}
