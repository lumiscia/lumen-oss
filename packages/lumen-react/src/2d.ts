import { runtime2d } from "@lumiscia/lumen-bindings/urls-2d";

import { createPlayer } from "./Player.js";

/** Previews a composition with the smaller 2D-only wasm, which reports 3D nodes as `node.unsupported`. */
export const Player = createPlayer(runtime2d);

export * from "./shared.js";
