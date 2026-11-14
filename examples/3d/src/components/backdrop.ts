import { nodes as n, type Color, type NodeValue } from "@lumiscia/lumen-sdk";

/** A frame-filling gradient from `top` down to `bottom`. */
export function gradient(top: Color, bottom: Color): NodeValue<"raster"> {
  return n.background({
    paint: {
      type: "linear",
      start: [0, 0],
      end: [0, 1],
      stops: [
        [0, top],
        [1, bottom],
      ],
    },
  });
}
