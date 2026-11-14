import { nodes as n, type NodeValue } from "@lumiscia/lumen-shared";

import { H, W } from "../theme.js";

/** The video at `source`, scaled to fill the frame and cropped to it. */
export function footage(source: string): NodeValue<"raster"> {
  return n.resize({
    width: W,
    height: H,
    mode: "fill",
    source: n.media_in({ kind: "video", source }),
  });
}
