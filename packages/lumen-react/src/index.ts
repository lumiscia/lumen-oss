import * as urls from "@lumiscia/lumen-bindings/urls";

import { createPlayer } from "./Player.js";

/** Previews a composition with the full wasm, which renders 2D and 3D nodes. */
export const Player = createPlayer({
  ...urls,
  wasmFile: "lumen_wasm_bg.wasm",
  workerFile: "worker.js",
});

export * from "./shared.js";
