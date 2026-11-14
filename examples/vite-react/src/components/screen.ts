import { nodes as n, type NodeValue } from "@lumiscia/lumen-shared";

import { footage } from "./footage.js";

/** The video at `source` on a plane behind the shapes. */
export function screen(source: string): NodeValue<"scene"> {
  return n.image_plane_3d({ width: 4, position: [0, 1.3, -2.5], source: footage(source) });
}
