import assert from "node:assert/strict";

import { LumenPreviewController, createLumenBindings } from "@lumiscia/lumen-bindings/node";

const { LumenPreviewController: Controller } = await createLumenBindings().preview();
assert.equal(Controller, LumenPreviewController);

const controller = new Controller();
controller.loadComposition(
  JSON.stringify({
    timeline: { fps: 30, duration_frames: 90 },
    render_settings: { width: 8, height: 4 },
    nodes: [
      { id: 1, type: "background", params: { width: 8, height: 4, paint: [0, 0, 0, 255] } },
      { id: 2, type: "media_output" },
    ],
    connections: [{ from_node: 1, from_port: "output", to_node: 2, to_port: "source" }],
  }),
);
assert.equal(controller.durationFrames(), 90);
assert.equal(controller.fps(), 30);
assert.equal(controller.width(), 8);
console.log("@lumiscia/lumen-bindings/node loaded the web wasm build");
