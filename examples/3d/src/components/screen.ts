import { nodes as n, sin, time, vec3, type NodeValue } from "@lumiscia/lumen-sdk";

import { SHEET } from "../media.js";
import { footage } from "./footage.js";

/**
 * The sheet model with the video at `source` in place of its `grid` material, unlit, as an image
 * plane shows a raster. It is stretched to the clip's 16:9 and swings about its vertical axis.
 */
export function screen(source: string): NodeValue<"scene"> {
  return n.model_3d({
    source: SHEET,
    texture_material: "grid",
    position: [0, -1, 0],
    rotation: vec3(0, sin(time.mul(1.2)).mul(28), 0),
    scale: [1.78, 1, 1],
    texture: footage(source),
  });
}
