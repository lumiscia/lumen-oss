import { nodes as n, type NodeValue } from "@lumiscia/lumen-sdk";

import { SANS, WHITE } from "../theme.js";

/** A line of white type at the lower left of the frame. */
export function title(content: string): NodeValue<"raster"> {
  return n.text({ content, font_family: SANS, font_size: 96, color: WHITE, position: [64, 520] });
}
