import * as urls from "@lumiscia/lumen-bindings/urls-2d";

import { createPlayer } from "./Player.js";

/** Previews a composition with the smaller 2D-only wasm, which reports 3D nodes as `node.unsupported`. */
export const Player = createPlayer({
  ...urls,
  wasmFile: "lumen_wasm_2d_bg.wasm",
  workerFile: "worker-2d.js",
});

export * from "./shared.js";
