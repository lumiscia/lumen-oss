import assert from "node:assert/strict";

import { LumenPreviewController, createLumenBindings } from "@lumiscia/lumen-bindings/node";

const { LumenPreviewController: Controller, check } = await createLumenBindings().preview();
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

const diagnostics = check(
  JSON.stringify({
    timeline: { fps: 30, duration_frames: 90 },
    render_settings: { width: 8, height: 4 },
    nodes: [
      { id: 1, type: "text", params: { font_family: "Nope Sans" } },
      { id: 2, type: "media_output" },
    ],
    connections: [{ from_node: 1, from_port: "output", to_node: 2, to_port: "source" }],
  }),
  [],
);
assert.deepEqual(
  diagnostics.map(({ severity, code, path, node }) => ({ severity, code, path, node })),
  [{ severity: "warning", code: "font.fallback", path: "/nodes/0/params/font_family", node: 1 }],
);
assert.equal(check("{").at(0)?.code, "document.syntax");
assert.throws(
  () => controller.loadComposition("{"),
  (error) => error instanceof Error && error.diagnostics[0]?.code === "document.syntax",
);

console.log("@lumiscia/lumen-bindings/node loaded the web wasm build");
