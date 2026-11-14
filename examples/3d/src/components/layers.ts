import { nodes as n, type Color, type NodeSource, type NodeValue } from "@lumiscia/lumen-sdk";

import { W } from "../theme.js";

/**
 * A 2D layer as a plane: its raster is composition-sized, so `width` sets how much of the scene it
 * spans and the height follows the 16:9 canvas.
 */
export function layer(
  source: NodeSource<"raster">,
  width: number,
  position: readonly [number, number, number],
): NodeValue<"scene"> {
  return n.image_plane_3d({ source, width, position });
}

/**
 * An ellipse wider than the canvas, centred at `centerY`, whose top edge is the ridge. A plane's
 * own edge shows if the camera can see it, so hills are wider than the view at their depth.
 */
export function hill(color: Color, centerY: number): NodeValue<"raster"> {
  const width = 2600;
  const height = 900;
  // A shape's `position` is its top-left corner.
  return n.shape({
    geometry_kind: "ellipse",
    width,
    height,
    position: [W / 2 - width / 2, centerY - height / 2],
    fill_paint: color,
  });
}
